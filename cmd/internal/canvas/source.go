package canvas

import (
	"bufio"
	"context"
	"crypto/hmac"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

// Sources bring events: a node with outputs only, whose events go along
// its edges as any output does. The canvas has four of its own — manual (a
// button), timer, files (a pattern of the workspace's files changing) and
// webhook (a request to a secret address) — and a plugin can declare more:
// a program the engine runs in the workspace, given its node's
// configuration on its input, that prints its events as lines of JSON:
//
//	{"type":"event","port":"out","key":"…","title":"…","text":"…","data":{…}}
//	{"type":"status","state":"ok"|"error","text":"…"}
//	{"type":"log","text":"…"}
//
// A polling source runs every interval and exits; a streaming one runs on,
// its input left open (its end says stop). An event with a key seen before
// is dropped; the keys seen are kept in the node's state directory
// (KOU_CANVAS_STATE_DIR), where the program keeps what it likes. A program
// that fails is run again later and later, from a second to a minute;
// three failures in a row show the node as failing.

// sourceInstance is what runs for a source node.
type sourceInstance interface {
	instance
	isSource()
}

// startSource starts a source node, unless its canvas is paused.
func (c *Canvas) startSource(n *Node) {
	// A plugin's source whose plugin is off starts all the same, and waits
	// for its plugin (processSource.run).
	def, ok := c.e.catalog(c.ws).source(n.Plugin, n.Preset)
	if !ok && n.Plugin == "" {
		c.setStatus(n.ID, StateError, fmt.Sprintf("no source %q", n.Preset))
		return
	}
	c.mu.Lock()
	live := c.doc.Live
	st := c.states[n.ID]
	c.mu.Unlock()
	if st == nil {
		return
	}
	if !live {
		c.setStatus(n.ID, StateStopped, "the canvas is paused")
		return
	}
	ctx, cancel := context.WithCancel(c.e.ctx)
	base := baseSource{c: c, id: n.ID, ctx: ctx, cancel: cancel, last: &lastEvent{}}
	var inst sourceInstance
	switch {
	case n.Plugin == "" && n.Preset == "manual":
		inst = &manualSource{baseSource: base}
	case n.Plugin == "" && n.Preset == "timer":
		inst = &timerSource{baseSource: base}
	case n.Plugin == "" && n.Preset == "files":
		inst = &filesSource{baseSource: base}
	case n.Plugin == "" && n.Preset == "webhook":
		inst = &webhookSource{baseSource: base}
	case ok && def.Source.Builtin:
		cancel()
		c.setStatus(n.ID, StateError, "the source "+n.Preset+" is not one the canvas has")
		return
	default:
		inst = &processSource{baseSource: base}
	}
	c.mu.Lock()
	if st := c.states[n.ID]; st == nil || c.closed {
		c.mu.Unlock()
		cancel()
		return
	} else {
		old := st.inst
		st.inst = inst
		if old != nil {
			defer old.stop()
		}
	}
	c.setStatusLocked(n.ID, StateIdle, "")
	c.mu.Unlock()
	if runner, ok := inst.(interface{ run(*Node) }); ok {
		go runner.run(n)
	}
}

// baseSource is what every source has.
type baseSource struct {
	c      *Canvas
	id     string
	ctx    context.Context
	cancel context.CancelFunc
	last   *lastEvent
}

// lastEvent is the text of a source's last event.
type lastEvent struct {
	mu   sync.Mutex
	text string
}

func (b *baseSource) isSource() {}

func (b *baseSource) deliver(*Message) error { return errors.New("a source takes no input") }

func (b *baseSource) read(what string, lines int) (string, error) {
	b.last.mu.Lock()
	defer b.last.mu.Unlock()
	return b.last.text, nil
}

func (b *baseSource) stop() { b.cancel() }

// fire gives an event.
func (b *baseSource) fire(out Output) {
	if b.ctx.Err() != nil {
		return
	}
	b.last.mu.Lock()
	b.last.text = out.Text
	b.last.mu.Unlock()
	b.c.emit(b.id, out)
}

// manualSource fires when the user presses its button.
type manualSource struct{ baseSource }

// fire is the button: text, or the node's, goes out.
func (s *manualSource) press(text string) {
	if strings.TrimSpace(text) == "" {
		if n := s.c.node(s.id); n != nil {
			text = n.configString("text")
		}
	}
	s.fire(Output{Port: "out", Title: "Fired", Text: text, Data: jsonOf(map[string]any{"fired_at": now()})})
}

// timerSource fires every so often, or at a time of the day.
type timerSource struct{ baseSource }

func (s *timerSource) run(n *Node) {
	every, err := time.ParseDuration(strings.TrimSpace(n.configString("every")))
	at := strings.TrimSpace(n.configString("at"))
	if at == "" && (err != nil || every <= 0) {
		every = 15 * time.Minute
	}
	if every > 0 && every < 10*time.Second {
		every = 10 * time.Second
	}
	text := n.configString("text")
	for {
		wait := every
		if at != "" {
			next, err := nextTimeOfDay(at, time.Now())
			if err != nil {
				s.c.setStatus(s.id, StateError, err.Error())
				return
			}
			wait = time.Until(next)
		}
		s.c.setStatus(s.id, StateIdle, "next at "+time.Now().Add(wait).Format("15:04:05"))
		select {
		case <-s.ctx.Done():
			return
		case t := <-time.After(wait):
			body := text
			if body == "" {
				body = "tick " + t.Format(time.RFC3339)
			}
			s.fire(Output{Port: "out", Title: "Tick", Text: body, Data: jsonOf(map[string]any{"time": t.UTC().Format(time.RFC3339)})})
		}
	}
}

// nextTimeOfDay is when a time of the day, HH:MM, comes next.
func nextTimeOfDay(at string, from time.Time) (time.Time, error) {
	clock, err := time.Parse("15:04", at)
	if err != nil {
		return time.Time{}, fmt.Errorf("at must be HH:MM")
	}
	next := time.Date(from.Year(), from.Month(), from.Day(), clock.Hour(), clock.Minute(), 0, 0, from.Location())
	if !next.After(from) {
		next = next.AddDate(0, 0, 1)
	}
	return next, nil
}

// filesSource fires when files of the workspace that match its pattern
// change.
type filesSource struct{ baseSource }

// Bounds of what a files source looks at.
const (
	filesLimit    = 20000
	filesInterval = 2 * time.Second
	filesSettle   = 600 * time.Millisecond
)

func (s *filesSource) run(n *Node) {
	pattern := strings.TrimSpace(n.configString("pattern"))
	if pattern == "" {
		pattern = "**/*"
	}
	interval := filesInterval
	if every, err := time.ParseDuration(n.configString("interval")); err == nil && every >= 400*time.Millisecond {
		interval = every
	}
	root := s.c.ws.Path
	before := fingerprint(root, pattern)
	var changed map[string]string
	var settle <-chan time.Time
	for {
		select {
		case <-s.ctx.Done():
			return
		case <-time.After(interval):
		case <-settle:
			settle = nil
			var added, removed, modified []string
			for name, what := range changed {
				switch what {
				case "added":
					added = append(added, name)
				case "removed":
					removed = append(removed, name)
				default:
					modified = append(modified, name)
				}
			}
			slices.Sort(added)
			slices.Sort(removed)
			slices.Sort(modified)
			all := append(append(append([]string(nil), modified...), added...), removed...)
			text := strings.Join(all, "\n")
			s.fire(Output{Port: "out", Title: fmt.Sprintf("%d files changed", len(all)), Text: text,
				Data: jsonOf(map[string]any{"changed": modified, "added": added, "removed": removed})})
			changed = nil
			continue
		}
		after := fingerprint(root, pattern)
		for name, sum := range after {
			if old, ok := before[name]; !ok {
				changed = mark(changed, name, "added")
			} else if old != sum {
				changed = mark(changed, name, "changed")
			}
		}
		for name := range before {
			if _, ok := after[name]; !ok {
				changed = mark(changed, name, "removed")
			}
		}
		if len(changed) > 0 && !mapsEqual(before, after) {
			settle = time.After(filesSettle)
		}
		before = after
	}
}

func mark(m map[string]string, name, what string) map[string]string {
	if m == nil {
		m = map[string]string{}
	}
	if m[name] == "added" && what == "changed" {
		return m
	}
	m[name] = what
	return m
}

func mapsEqual(a, b map[string]string) bool {
	if len(a) != len(b) {
		return false
	}
	for key, value := range a {
		if b[key] != value {
			return false
		}
	}
	return true
}

// fingerprint is the size and time of the files under root that match a
// pattern, by their paths relative to root.
func fingerprint(root, pattern string) map[string]string {
	out := map[string]string{}
	_ = filepath.WalkDir(root, func(p string, entry fs.DirEntry, err error) error {
		if err != nil {
			return nil
		}
		name := entry.Name()
		if entry.IsDir() {
			if p != root && (name == ".git" || name == "node_modules" || name == ".harness" || strings.HasPrefix(name, ".")) {
				return filepath.SkipDir
			}
			return nil
		}
		relative, err := filepath.Rel(root, p)
		if err != nil || !globMatch(pattern, filepath.ToSlash(relative)) {
			return nil
		}
		info, err := entry.Info()
		if err != nil {
			return nil
		}
		out[filepath.ToSlash(relative)] = fmt.Sprintf("%d/%d", info.Size(), info.ModTime().UnixNano())
		if len(out) >= filesLimit {
			return filepath.SkipAll
		}
		return nil
	})
	return out
}

// globMatch matches a slash-separated path against a pattern in which **
// stands for any number of directories.
func globMatch(pattern, name string) bool {
	patterns := strings.Split(pattern, "/")
	names := strings.Split(name, "/")
	var match func(p, n []string) bool
	match = func(p, n []string) bool {
		for len(p) > 0 {
			if p[0] == "**" {
				for i := 0; i <= len(n); i++ {
					if match(p[1:], n[i:]) {
						return true
					}
				}
				return false
			}
			if len(n) == 0 {
				return false
			}
			if ok, err := path.Match(p[0], n[0]); err != nil || !ok {
				return false
			}
			p, n = p[1:], n[1:]
		}
		return len(n) == 0
	}
	return match(patterns, names)
}

// webhookSource fires when its address is posted to.
type webhookSource struct{ baseSource }

// maxHook bounds what a webhook takes.
const maxHook = 1 << 20

// receive takes a request to the hook in.
func (s *webhookSource) receive(body []byte, contentType string) {
	text := string(body)
	var data jsontext.Value
	if value := jsontext.Value(body); value.IsValid() && (value.Kind() == '{' || value.Kind() == '[') {
		data = append(jsontext.Value(nil), body...)
		var fields map[string]any
		if json.Unmarshal(body, &fields) == nil {
			if t, ok := fields["text"].(string); ok {
				text = t
			}
		}
	}
	s.fire(Output{Port: "out", Title: "Received", Text: text, Data: data})
}

// hookID is the secret part of a webhook source's address.
func (c *Canvas) hookID(node string) string {
	mac := hmac.New(sha256.New, c.e.secret)
	mac.Write([]byte("hook|" + c.ws.ID + "|" + c.id + "|" + node))
	return base64.RawURLEncoding.EncodeToString(mac.Sum(nil))[:32]
}

// findHook finds the webhook source an address names.
func (e *Engine) findHook(hook string) (*Canvas, *webhookSource) {
	for _, c := range e.all() {
		c.mu.Lock()
		for _, n := range c.doc.Nodes {
			if n.Kind != KindSource || n.Preset != "webhook" || n.Plugin != "" {
				continue
			}
			if hmac.Equal([]byte(c.hookID(n.ID)), []byte(hook)) {
				var src *webhookSource
				if st := c.states[n.ID]; st != nil {
					src, _ = st.inst.(*webhookSource)
				}
				c.mu.Unlock()
				return c, src
			}
		}
		c.mu.Unlock()
	}
	return nil, nil
}

// processSource runs a plugin's program.
type processSource struct {
	baseSource

	logMu sync.Mutex
	logs  []string
}

const (
	maxSeen     = 10000
	maxLogLines = 100
	maxEvent    = 1 << 20
	// pluginWait is how often a source whose plugin is off looks for it
	// again, and a streaming one whether its plugin is still on.
	pluginWait = catalogFor
)

// errPluginOff is why a streaming source's program was stopped: its
// plugin went off, or changed what it runs.
var errPluginOff = errors.New("its plugin is off")

func (s *processSource) run(n *Node) {
	state := stateDir(s.c.ws.Path, s.c.id, s.id)
	if err := os.MkdirAll(state, 0o700); err != nil {
		s.c.setStatus(s.id, StateError, err.Error())
		return
	}
	seen := loadSeen(state)
	failures := 0
	backoff := time.Second
	off := false
	for s.ctx.Err() == nil {
		// The plugin is looked up for every run: one turned off, removed or
		// no longer trusted stops its sources until it is back, and one
		// changed runs as it is now.
		def, ok := s.c.e.catalog(s.c.ws).source(n.Plugin, n.Preset)
		if !ok {
			if !off {
				off = true
				s.log("its plugin is off: the source waits for it")
				s.c.setStatus(s.id, StateStopped, "its plugin is off")
			}
			if !s.sleep(pluginWait) {
				return
			}
			continue
		}
		if off {
			off = false
			s.c.setStatus(s.id, StateIdle, "")
		}
		command, err := sourceCommand(def)
		if err != nil {
			s.c.setStatus(s.id, StateError, err.Error())
			return
		}
		stream := def.Source.Mode == "stream"
		interval := def.Source.SourceInterval()
		if every, err := time.ParseDuration(n.configString("interval")); err == nil && every >= interval {
			interval = every
		}
		first := !fileExists(filepath.Join(state, "seen.json"))
		err = s.runOnce(n, def, command, state, seen, first, stream)
		if s.ctx.Err() != nil {
			return
		}
		_ = saveSeen(state, seen)
		if errors.Is(err, errPluginOff) {
			continue
		}
		wait := interval
		if err != nil {
			failures++
			s.log("the source failed: " + err.Error())
			if failures >= 3 {
				s.c.setStatus(s.id, StateError, err.Error())
			}
			wait = backoff
			backoff = min(backoff*2, time.Minute)
		} else {
			failures, backoff = 0, time.Second
			if stream {
				wait = time.Second
			}
		}
		select {
		case <-s.ctx.Done():
			return
		case <-time.After(wait):
		}
	}
}

// runOnce runs the program once: a poll, or a stream until it ends — or
// until its plugin goes off or changes what it runs.
func (s *processSource) runOnce(n *Node, def sourceDef, command []string, state string, seen *seenKeys, first, stream bool) error {
	ctx, cancel := context.WithCancelCause(s.ctx)
	defer cancel(nil)
	if stream {
		go s.watch(ctx, cancel, n.Plugin, n.Preset, def)
	} else {
		var stop context.CancelFunc
		ctx, stop = context.WithTimeout(ctx, 5*time.Minute)
		defer stop()
	}
	cmd := exec.CommandContext(ctx, command[0], command[1:]...)
	cmd.Dir = s.c.ws.Path
	cmd.Env = append(os.Environ(), s.c.e.env(s.c, s.id, scopeSource, n.Runtime.Epoch)...)
	cmd.Env = append(cmd.Env,
		"KOU_CANVAS_STATE_DIR="+state,
		"KOU_CONVEYOR_PLUGIN_DIR="+def.Owner.Directory,
		"KOU_CONVEYOR_WORKSPACE="+s.c.ws.Path,
	)
	cmd.WaitDelay = 2 * time.Second
	stdin, err := cmd.StdinPipe()
	if err != nil {
		return err
	}
	stdout, err := cmd.StdoutPipe()
	if err != nil {
		return err
	}
	stderr, err := cmd.StderrPipe()
	if err != nil {
		return err
	}
	if err := cmd.Start(); err != nil {
		return err
	}
	config := n.Config
	if len(config) == 0 {
		config = jsontext.Value("{}")
	}
	input, _ := json.Marshal(map[string]any{"config": config, "node": s.id, "canvas": s.c.id, "first_run": first})
	_, _ = stdin.Write(append(input, '\n'))
	if !stream {
		stdin.Close()
	} else {
		go func() {
			<-ctx.Done()
			stdin.Close()
		}()
	}
	var wg sync.WaitGroup
	wg.Add(1)
	go func() {
		defer wg.Done()
		scanner := bufio.NewScanner(stderr)
		scanner.Buffer(make([]byte, 64<<10), maxEvent)
		for scanner.Scan() {
			s.log(scanner.Text())
		}
	}()
	statusErr := ""
	scanner := bufio.NewScanner(stdout)
	scanner.Buffer(make([]byte, 64<<10), maxEvent)
	for scanner.Scan() {
		var line struct {
			Type  string         `json:"type"`
			Port  string         `json:"port"`
			Key   string         `json:"key"`
			Title string         `json:"title"`
			Text  string         `json:"text"`
			Data  jsontext.Value `json:"data"`
			State string         `json:"state"`
		}
		if json.Unmarshal(scanner.Bytes(), &line) != nil {
			s.log("not a line of JSON: " + previewOf(scanner.Text()))
			continue
		}
		switch line.Type {
		case "event":
			if line.Key != "" && !seen.add(line.Key) {
				continue
			}
			port := line.Port
			if port == "" {
				port = "out"
			}
			if !slices.ContainsFunc(def.Source.Outputs, func(p plugin.CanvasPort) bool { return p.ID == port }) {
				s.log("an event on a port the source does not declare: " + port)
				continue
			}
			s.fire(Output{Port: port, Title: line.Title, Text: line.Text, Data: line.Data})
		case "status":
			if line.State == "error" {
				statusErr = line.Text
				s.c.setStatus(s.id, StateError, line.Text)
			} else {
				statusErr = ""
				s.c.setStatus(s.id, StateIdle, line.Text)
			}
		case "log":
			s.log(line.Text)
		}
	}
	wg.Wait()
	err = cmd.Wait()
	if errors.Is(context.Cause(ctx), errPluginOff) {
		return errPluginOff
	}
	if err == nil && statusErr != "" {
		return errors.New(statusErr)
	}
	if err != nil && ctx.Err() != nil && s.ctx.Err() == nil && !stream {
		return fmt.Errorf("it ran over 5 minutes")
	}
	return err
}

// watch stops a streaming source's program when its plugin goes off, or
// changes what it runs.
func (s *processSource) watch(ctx context.Context, stop context.CancelCauseFunc, pluginName, preset string, def sourceDef) {
	ticker := time.NewTicker(pluginWait)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
			now, ok := s.c.e.catalog(s.c.ws).source(pluginName, preset)
			if !ok || now.Owner.Directory != def.Owner.Directory || now.Source.Mode != def.Source.Mode || !slices.Equal(now.Source.Run, def.Source.Run) {
				stop(errPluginOff)
				return
			}
		}
	}
}

// sourceCommand is what runs a plugin's source.
func sourceCommand(def sourceDef) ([]string, error) {
	command, err := def.Owner.SourceCommand(def.Source)
	switch {
	case err != nil:
		return nil, fmt.Errorf("cannot run the source: %w", err)
	case len(command) == 0:
		return nil, errors.New("the source runs nothing")
	}
	if def.Owner.Directory == "" {
		for _, arg := range def.Source.Run {
			if strings.HasPrefix(arg, "./") || strings.HasPrefix(arg, "../") {
				return nil, errors.New("the plugin's files are not on disk: its source cannot run")
			}
		}
	}
	return command, nil
}

// sleep waits a while, unless the source stops first: false if it did.
func (s *processSource) sleep(d time.Duration) bool {
	select {
	case <-s.ctx.Done():
		return false
	case <-time.After(d):
		return true
	}
}

func (s *processSource) log(text string) {
	s.logMu.Lock()
	s.logs = append(s.logs, text)
	if len(s.logs) > maxLogLines {
		s.logs = s.logs[len(s.logs)-maxLogLines:]
	}
	s.logMu.Unlock()
	s.c.mu.Lock()
	if s.c.states[s.id] != nil {
		s.c.hub.publish(map[string]any{"type": "log", "node": s.id, "text": previewOf(text), "at": now()})
	}
	s.c.mu.Unlock()
}

func (s *processSource) read(what string, lines int) (string, error) {
	if what == "tail" || what == "screen" {
		s.logMu.Lock()
		defer s.logMu.Unlock()
		logs := s.logs
		if lines > 0 && len(logs) > lines {
			logs = logs[len(logs)-lines:]
		}
		return strings.Join(logs, "\n"), nil
	}
	return s.baseSource.read(what, lines)
}

// seenKeys are the keys of the events a source gave, the latest last.
type seenKeys struct {
	order []string
	set   map[string]bool
}

func (k *seenKeys) add(key string) bool {
	if k.set[key] {
		return false
	}
	k.set[key] = true
	k.order = append(k.order, key)
	if len(k.order) > maxSeen {
		for _, old := range k.order[:len(k.order)-maxSeen] {
			delete(k.set, old)
		}
		k.order = append(k.order[:0:0], k.order[len(k.order)-maxSeen:]...)
	}
	return true
}

func loadSeen(dir string) *seenKeys {
	k := &seenKeys{set: map[string]bool{}}
	data, err := os.ReadFile(filepath.Join(dir, "seen.json"))
	if err == nil {
		var keys []string
		if json.Unmarshal(data, &keys) == nil {
			for _, key := range keys {
				k.add(key)
			}
		}
	}
	return k
}

func saveSeen(dir string, k *seenKeys) error {
	data, err := json.Marshal(k.order)
	if err != nil {
		return err
	}
	if k.order == nil {
		data = []byte("[]")
	}
	return writeAtomic(filepath.Join(dir, "seen.json"), data, 0o600)
}

func fileExists(path string) bool {
	_, err := os.Stat(path)
	return err == nil
}

func removeAll(path string) error { return os.RemoveAll(path) }

// readBody reads at most limit bytes of a request's body.
func readBody(body io.Reader, limit int64) ([]byte, error) {
	data, err := io.ReadAll(io.LimitReader(body, limit+1))
	if err != nil {
		return nil, err
	}
	if int64(len(data)) > limit {
		return nil, errInvalid(fmt.Sprintf("the body is over %d KiB", limit>>10))
	}
	return data, nil
}
