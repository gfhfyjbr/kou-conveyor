package canvas

import (
	"bufio"
	"bytes"
	"crypto/sha256"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"path/filepath"
	"sort"
	"strconv"
	"strings"
	"sync"
	"time"
	"unicode/utf8"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/transcript"
)

const (
	defaultTranscriptLimit = 50
	maxTranscriptLimit     = 500
	defaultMessageLimit    = 8 << 10
	defaultOutputLimit     = 4 << 10
	maxFieldLimit          = 1 << 20
)

// transcriptOptions are the bounded, incremental transcript query. The
// cursor is kept separately because "last" belongs to the reader and target,
// not to the session file.
type transcriptOptions struct {
	After        int64
	Limit        int
	Kinds        map[string]bool
	Tools        map[string]bool
	Since        *time.Time
	Until        *time.Time
	MessageLimit int
	OutputLimit  int
}

type transcriptCursor struct {
	Offset int64 `json:"offset"`
}

type rawFeedEntry struct {
	ID        string
	Kind      string
	At        time.Time
	Text      string
	Detail    string
	Phase     string
	State     string
	Model     string
	Forced    bool
	Tool      *cockpit.Tool
	ChangedAt int64
	Changed   int64
}

type rawFeed struct {
	path           string
	info           fs.FileInfo
	offset         int64
	transcript     *cockpit.Transcript
	changed        map[string]int64
	serial         map[string]int64
	nextSerial     int64
	skipped        int
	fingerprint    [sha256.Size]byte
	hasFingerprint bool
}

type rawFeedCache struct {
	mu    sync.Mutex
	max   int
	files map[string]*rawFeed
	lru   []string
}

func newRawFeedCache() *rawFeedCache {
	return &rawFeedCache{max: 8, files: make(map[string]*rawFeed)}
}

func newRawFeed(path string) *rawFeed {
	return &rawFeed{
		path: path, transcript: cockpit.NewTranscript(),
		changed: make(map[string]int64), serial: make(map[string]int64),
	}
}

func (cache *rawFeedCache) file(path string) *rawFeed {
	if feed := cache.files[path]; feed != nil {
		cache.touch(path)
		return feed
	}
	feed := newRawFeed(path)
	cache.files[path] = feed
	cache.lru = append(cache.lru, path)
	for len(cache.lru) > cache.max {
		old := cache.lru[0]
		cache.lru = cache.lru[1:]
		delete(cache.files, old)
	}
	return feed
}

func (cache *rawFeedCache) touch(path string) {
	for i, item := range cache.lru {
		if item == path {
			cache.lru = append(cache.lru[:i], cache.lru[i+1:]...)
			break
		}
	}
	cache.lru = append(cache.lru, path)
}

// read folds only complete new lines. It holds the cache lock while folding
// because cockpit.Transcript is deliberately not safe for concurrent use.
func (cache *rawFeedCache) read(path string, after int64, options transcriptOptions) ([]rawFeedEntry, int64, int, error) {
	cache.mu.Lock()
	defer cache.mu.Unlock()
	feed := cache.file(path)
	fileReset, err := feed.refresh()
	if err != nil {
		return nil, 0, 0, err
	}
	reset := 0
	if fileReset {
		after = 0
		reset = 1
	} else if after < 0 || after > feed.offset || !cursorBoundary(path, after) {
		after = 0
		reset = 1
	}
	entries := feed.entries(after, options)
	next := feed.offset
	if len(entries) >= options.Limit {
		next = entries[len(entries)-1].ChangedAt
		for _, entry := range feed.entriesAfter(after, options) {
			if entry.ChangedAt != next {
				break
			}
			next = entry.ChangedAt
		}
	}
	return entries, next, reset, nil
}

// refresh adds complete JSONL records after offset. An incomplete last record
// is deliberately left for the next call, since a runner may still be writing
// it. Replacements and truncations start a fresh fold.
func (feed *rawFeed) refresh() (bool, error) {
	info, err := os.Stat(feed.path)
	if err != nil {
		return false, err
	}
	fileReset := false
	if feed.info != nil {
		replaced := !os.SameFile(feed.info, info)
		rewritten := false
		if info.Size() == feed.offset && feed.hasFingerprint {
			current, fingerprintErr := fileFingerprint(feed.path, info.Size())
			if fingerprintErr != nil {
				return false, fingerprintErr
			}
			rewritten = current != feed.fingerprint
		}
		if info.Size() < feed.offset || replaced || rewritten ||
			(info.Size() == feed.offset && !info.ModTime().Equal(feed.info.ModTime())) {
			feed.reset()
			fileReset = true
		}
	}
	file, err := os.Open(feed.path)
	if err != nil {
		return false, err
	}
	defer file.Close()
	if _, err := file.Seek(feed.offset, io.SeekStart); err != nil {
		return false, err
	}
	reader := bufio.NewReaderSize(file, 256<<10)
	position := feed.offset
	for {
		line, readErr := reader.ReadBytes('\n')
		if len(line) == 0 && errors.Is(readErr, io.EOF) {
			break
		}
		if !bytes.HasSuffix(line, []byte{'\n'}) {
			// This is the partial tail. Do not advance over it.
			break
		}
		end := position + int64(len(line))
		line = line[:len(line)-1]
		changed, applyErr := feed.transcript.Apply(line)
		if applyErr != nil {
			feed.skipped++
		} else {
			for _, entry := range changed {
				if entry == nil {
					continue
				}
				id := entry.ID
				feed.nextSerial++
				feed.changed[id] = end
				feed.serial[id] = feed.nextSerial
			}
		}
		position = end
		if readErr != nil {
			if errors.Is(readErr, io.EOF) {
				break
			}
			return false, readErr
		}
	}
	feed.offset = position
	feed.info = info
	fingerprint, err := fileFingerprint(feed.path, info.Size())
	if err != nil {
		return false, err
	}
	feed.fingerprint, feed.hasFingerprint = fingerprint, true
	return fileReset, nil
}

func (feed *rawFeed) reset() {
	feed.offset = 0
	feed.transcript = cockpit.NewTranscript()
	feed.changed = make(map[string]int64)
	feed.serial = make(map[string]int64)
	feed.nextSerial = 0
	feed.skipped = 0
	feed.info = nil
	feed.fingerprint, feed.hasFingerprint = [sha256.Size]byte{}, false
}

const fingerprintBytes = 512

func fileFingerprint(path string, size int64) ([sha256.Size]byte, error) {
	file, err := os.Open(path)
	if err != nil {
		return [sha256.Size]byte{}, err
	}
	defer file.Close()
	hash := sha256.New()
	var head [fingerprintBytes]byte
	n, err := file.ReadAt(head[:], 0)
	if err != nil && !errors.Is(err, io.EOF) {
		return [sha256.Size]byte{}, err
	}
	hash.Write(head[:n])
	if size > fingerprintBytes {
		var tail [fingerprintBytes]byte
		n, err = file.ReadAt(tail[:], max(0, size-fingerprintBytes))
		if err != nil && !errors.Is(err, io.EOF) {
			return [sha256.Size]byte{}, err
		}
		hash.Write(tail[:n])
	}
	var result [sha256.Size]byte
	copy(result[:], hash.Sum(nil))
	return result, nil
}

func (feed *rawFeed) entriesAfter(after int64, options transcriptOptions) []rawFeedEntry {
	var entries []rawFeedEntry
	for id, changedAt := range feed.changed {
		if changedAt <= after {
			continue
		}
		entry := feed.transcript.Entry(id)
		if entry == nil {
			continue
		}
		raw := rawFeedEntryOf(entry, changedAt, feed.serial[id], options)
		if !matchesTranscript(raw, options) {
			continue
		}
		entries = append(entries, raw)
	}
	sort.Slice(entries, func(i, j int) bool {
		if entries[i].ChangedAt != entries[j].ChangedAt {
			return entries[i].ChangedAt < entries[j].ChangedAt
		}
		return entries[i].Changed < entries[j].Changed
	})
	return entries
}

func (feed *rawFeed) entries(after int64, options transcriptOptions) []rawFeedEntry {
	all := feed.entriesAfter(after, options)
	if len(all) <= options.Limit {
		return all
	}
	// Never split entries changed by one JSONL record: the cursor is a byte
	// offset, so doing so would make the remaining entry unobservable.
	cut := options.Limit
	last := all[cut-1].ChangedAt
	for cut < len(all) && all[cut].ChangedAt == last {
		cut++
	}
	return all[:cut]
}

func rawFeedEntryOf(entry *cockpit.Entry, changedAt, serial int64, options transcriptOptions) rawFeedEntry {
	return rawFeedEntry{
		ID: entry.ID, Kind: rawKind(entry), At: entry.At,
		Text: clipText(entry.Text, options.MessageLimit), Detail: clipText(entry.Detail, options.MessageLimit),
		Phase: entry.Phase, State: entry.State, Model: entry.Model, Forced: entry.Forced,
		Tool: entry.Tool, ChangedAt: changedAt, Changed: serial,
	}
}

func rawKind(entry *cockpit.Entry) string {
	if entry == nil {
		return ""
	}
	if entry.Kind == cockpit.KindNotice {
		if strings.HasPrefix(entry.Text, "Context compacted") || strings.HasPrefix(entry.Text, "Context not compacted") {
			return "compaction"
		}
		return "notice"
	}
	if entry.Kind == cockpit.KindUser {
		text := strings.TrimSpace(entry.Text)
		switch {
		case strings.HasPrefix(text, "[harness] Progress report"), strings.HasPrefix(text, "[harness] Heartbeat"), strings.HasPrefix(text, "Heartbeat:"):
			return "heartbeat"
		case strings.HasPrefix(text, "[canvas] from "), strings.HasPrefix(text, "[canvas] reply from "):
			return "peer"
		}
	}
	return entry.Kind
}

func matchesTranscript(entry rawFeedEntry, options transcriptOptions) bool {
	if len(options.Kinds) > 0 && !options.Kinds[entry.Kind] {
		return false
	}
	if len(options.Tools) > 0 {
		if entry.Tool == nil || !options.Tools[strings.ToLower(entry.Tool.Name)] {
			return false
		}
	}
	if options.Since != nil && entry.At.Before(*options.Since) {
		return false
	}
	if options.Until != nil && entry.At.After(*options.Until) {
		return false
	}
	return true
}

func cursorBoundary(path string, offset int64) bool {
	if offset == 0 {
		return true
	}
	if offset < 0 {
		return false
	}
	file, err := os.Open(path)
	if err != nil {
		return false
	}
	defer file.Close()
	if _, err := file.Seek(offset-1, io.SeekStart); err != nil {
		return false
	}
	var one [1]byte
	_, err = io.ReadFull(file, one[:])
	return err == nil && one[0] == '\n'
}

func clipText(text string, limit int) string {
	if limit <= 0 || len(text) <= limit {
		return text
	}
	head := limit / 2
	tail := limit - head
	head = utf8Boundary(text, head)
	tailStart := len(text) - tail
	tailStart = utf8BoundaryAfter(text, tailStart)
	removed := len(text) - tailStart + head
	return text[:head] + fmt.Sprintf("\n[… %d bytes cut]\n", removed) + text[tailStart:]
}

func utf8Boundary(text string, at int) int {
	at = min(max(at, 0), len(text))
	for at > 0 && at < len(text) && !utf8.RuneStart(text[at]) {
		at--
	}
	return at
}

func utf8BoundaryAfter(text string, at int) int {
	at = min(max(at, 0), len(text))
	for at < len(text) && !utf8.RuneStart(text[at]) {
		at++
	}
	return at
}

type rawFile struct {
	Path string `json:"path"`
	Size int64  `json:"size"`
}

func fullOutput(text string) *rawFile {
	const marker = "complete output in "
	at := strings.Index(text, marker)
	if at < 0 {
		return nil
	}
	rest := text[at+len(marker):]
	if end := strings.Index(rest, "..."); end >= 0 {
		rest = rest[:end]
	}
	if end := strings.IndexByte(rest, '\n'); end >= 0 {
		rest = rest[:end]
	}
	path := strings.TrimSpace(rest)
	if path == "" {
		return nil
	}
	info, err := os.Stat(path)
	if err != nil {
		return &rawFile{Path: path}
	}
	return &rawFile{Path: path, Size: info.Size()}
}

type rawTool struct {
	CallID     string    `json:"call_id"`
	Name       string    `json:"name,omitempty"`
	Input      string    `json:"input,omitempty"`
	State      string    `json:"state"`
	ExitCode   *int      `json:"exit_code,omitempty"`
	Output     string    `json:"output,omitempty"`
	Stderr     string    `json:"stderr,omitempty"`
	Error      string    `json:"error,omitempty"`
	Started    time.Time `json:"started,omitzero"`
	Finished   time.Time `json:"finished,omitzero"`
	FullOutput *rawFile  `json:"full_output,omitempty"`
	FullStderr *rawFile  `json:"full_stderr,omitempty"`
}

type transcriptEntry struct {
	ID        string    `json:"id"`
	Kind      string    `json:"kind"`
	At        time.Time `json:"at,omitzero"`
	Text      string    `json:"text,omitempty"`
	Detail    string    `json:"detail,omitempty"`
	Phase     string    `json:"phase,omitempty"`
	State     string    `json:"state,omitempty"`
	Model     string    `json:"model,omitempty"`
	Forced    bool      `json:"forced,omitempty"`
	Tool      *rawTool  `json:"tool,omitempty"`
	ChangedAt int64     `json:"changed_at"`
}

func (entry rawFeedEntry) public(options transcriptOptions) transcriptEntry {
	out := transcriptEntry{
		ID: entry.ID, Kind: entry.Kind, At: entry.At,
		Text: clipText(entry.Text, options.MessageLimit), Detail: clipText(entry.Detail, options.MessageLimit),
		Phase: entry.Phase, State: entry.State, Model: entry.Model, Forced: entry.Forced,
		ChangedAt: entry.ChangedAt,
	}
	if entry.Tool != nil {
		out.Tool = &rawTool{
			CallID: entry.Tool.CallID, Name: entry.Tool.Name,
			Input: clipText(entry.Tool.Input, options.MessageLimit), State: entry.Tool.State,
			ExitCode: entry.Tool.ExitCode, Output: clipText(entry.Tool.Output, options.OutputLimit),
			Stderr: clipText(entry.Tool.Stderr, options.OutputLimit), Error: clipText(entry.Tool.Error, options.OutputLimit),
			Started: entry.Tool.Started, Finished: entry.Tool.Finished,
			FullOutput: fullOutput(entry.Tool.Output), FullStderr: fullOutput(entry.Tool.Stderr),
		}
	}
	return out
}

func parseTranscriptOptions(q map[string][]string) (transcriptOptions, error) {
	get := func(name string) string {
		values := q[name]
		if len(values) == 0 {
			return ""
		}
		return values[0]
	}
	options := transcriptOptions{Limit: defaultTranscriptLimit, MessageLimit: defaultMessageLimit, OutputLimit: defaultOutputLimit}
	var err error
	if value := strings.TrimSpace(get("limit")); value != "" {
		options.Limit, err = strconv.Atoi(value)
		if err != nil || options.Limit < 1 {
			return options, fmt.Errorf("limit is a positive integer")
		}
		options.Limit = min(options.Limit, maxTranscriptLimit)
	}
	for _, field := range []struct {
		name string
		to   *int
	}{
		{"message_limit", &options.MessageLimit}, {"message_bytes", &options.MessageLimit},
		{"output_limit", &options.OutputLimit}, {"output_bytes", &options.OutputLimit},
	} {
		if value := strings.TrimSpace(get(field.name)); value != "" {
			*field.to, err = strconv.Atoi(value)
			if err != nil || *field.to < 1 {
				return options, fmt.Errorf("%s is a positive integer", field.name)
			}
			*field.to = min(*field.to, maxFieldLimit)
		}
	}
	options.Kinds, err = parseFilter(get("kinds"), allowedTranscriptKinds, "kinds")
	if err != nil {
		return options, err
	}
	options.Tools, err = parseFilter(get("tools"), nil, "tools")
	if err != nil {
		return options, err
	}
	for _, field := range []struct {
		name string
		to   **time.Time
	}{
		{"since", &options.Since}, {"until", &options.Until},
	} {
		if value := strings.TrimSpace(get(field.name)); value != "" {
			at, parseErr := time.Parse(time.RFC3339Nano, value)
			if parseErr != nil {
				return options, fmt.Errorf("%s is RFC3339 time", field.name)
			}
			*field.to = &at
		}
	}
	return options, nil
}

var allowedTranscriptKinds = map[string]bool{
	cockpit.KindUser: true, cockpit.KindAssistant: true, cockpit.KindReasoning: true,
	cockpit.KindTool: true, "notice": true, "error": true, "compaction": true,
	"heartbeat": true, "peer": true,
}

func parseFilter(value string, allowed map[string]bool, name string) (map[string]bool, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return nil, nil
	}
	result := map[string]bool{}
	for _, item := range strings.Split(value, ",") {
		item = strings.ToLower(strings.TrimSpace(item))
		if item == "" {
			continue
		}
		if allowed != nil && !allowed[item] {
			return nil, fmt.Errorf("unknown %s %q", name, item)
		}
		result[item] = true
	}
	return result, nil
}

func parseTranscriptAfter(value string) (int64, bool, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return 0, false, nil
	}
	if value == "last" {
		return 0, true, nil
	}
	offset, err := strconv.ParseInt(value, 10, 64)
	if err != nil || offset < 0 {
		return 0, false, fmt.Errorf("after is a cursor, a byte offset, or last")
	}
	return offset, false, nil
}

func transcriptCursorPath(dir string) string {
	return filepath.Join(dir, "transcript-cursors.json")
}

func loadTranscriptCursors(dir string) map[string]int64 {
	data, err := os.ReadFile(transcriptCursorPath(dir))
	if err != nil {
		return map[string]int64{}
	}
	var cursors map[string]int64
	if json.Unmarshal(data, &cursors) != nil || cursors == nil {
		return map[string]int64{}
	}
	return cursors
}

func (c *Canvas) saveTranscriptCursor(reader, target string, offset int64) error {
	key := reader + "\x00" + target
	c.mu.Lock()
	if c.transcriptCursors == nil {
		c.transcriptCursors = map[string]int64{}
	}
	c.transcriptCursors[key] = offset
	data, err := json.Marshal(c.transcriptCursors)
	if err == nil {
		err = writeAtomic(transcriptCursorPath(c.dir), append(data, '\n'), 0o600)
	}
	c.mu.Unlock()
	return err
}

func (c *Canvas) transcriptCursor(reader, target string) int64 {
	c.mu.Lock()
	defer c.mu.Unlock()
	return c.transcriptCursors[reader+"\x00"+target]
}

func (c *Canvas) transcriptSession(node string) (string, error) {
	n := c.node(node)
	if n == nil {
		return "", errNotFound("no such node")
	}
	if n.Kind != KindAgent {
		return "", errInvalid("the node is not an agent")
	}
	if n.Runtime.Session == "" {
		return "", errConflict("the agent has no session yet")
	}
	if !cockpit.ValidSessionID(n.Runtime.Session) {
		return "", errInvalid("the node's session ID is invalid")
	}
	path := cockpit.SessionPath(c.ws.SessionDir, n.Runtime.Session)
	if _, err := os.Stat(path); err != nil {
		return "", err
	}
	return path, nil
}

func (c *Canvas) canObserveTranscript(from, target string) bool {
	if from == target {
		return true
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	caller, targetNode := c.doc.node(from), c.doc.node(target)
	if caller == nil || targetNode == nil {
		return false
	}
	if targetNode.CreatedBy == from {
		return true
	}
	for _, edge := range c.doc.Edges {
		if edge.From.Node == from && edge.To.Node == target || edge.From.Node == target && edge.To.Node == from {
			return true
		}
	}
	return false
}

func includeFile(path string) (*rawFile, string, error) {
	data, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, fs.ErrNotExist) {
			return nil, "", nil
		}
		return nil, "", err
	}
	info, err := os.Stat(path)
	if err != nil {
		return nil, "", err
	}
	return &rawFile{Path: path, Size: info.Size()}, string(data), nil
}

func lastSummaries(text string, count int) string {
	parts := strings.Split(text, "\n## Compaction at ")
	if len(parts) <= count+1 {
		return text
	}
	return "## Compaction at " + strings.Join(parts[len(parts)-count:], "\n## Compaction at ")
}

func (c *Canvas) transcript(node string, q map[string][]string, reader string) (map[string]any, error) {
	options, err := parseTranscriptOptions(q)
	if err != nil {
		return nil, errInvalid(err.Error())
	}
	after, last, err := parseTranscriptAfter(firstQuery(q, "after"))
	if err != nil {
		return nil, errInvalid(err.Error())
	}
	if last {
		after = c.transcriptCursor(reader, node)
	}
	path, err := c.transcriptSession(node)
	if err != nil {
		return nil, err
	}
	entries, next, reset, err := c.e.raw.read(path, after, options)
	if err != nil {
		return nil, err
	}
	if last {
		if err := c.saveTranscriptCursor(reader, node, next); err != nil {
			return nil, fmt.Errorf("save transcript cursor: %w", err)
		}
	}
	public := make([]transcriptEntry, 0, len(entries))
	for _, entry := range entries {
		public = append(public, entry.public(options))
	}
	result := map[string]any{
		"node": node, "session": filepath.Base(strings.TrimSuffix(path, ".session.jsonl")),
		"cursor": strconv.FormatInt(after, 10), "next": strconv.FormatInt(next, 10),
		"entries": public,
	}
	if reset != 0 {
		result["cursor_reset"] = true
		result["cursor_reset_reason"] = "the cursor was not at a complete JSONL record"
	}
	if q["include"] != nil {
		include := map[string]bool{}
		for _, item := range strings.Split(firstQuery(q, "include"), ",") {
			include[strings.ToLower(strings.TrimSpace(item))] = true
		}
		if include["notes"] {
			file, text, includeErr := includeFile(filepath.Join(c.ws.SessionDir, result["session"].(string)+".notes.md"))
			if includeErr != nil {
				return nil, includeErr
			}
			if file != nil {
				result["notes"] = map[string]any{"file": file, "text": text}
			}
		}
		if include["summaries"] {
			path := filepath.Join(c.ws.SessionDir, result["session"].(string)+".summaries.md")
			file, text, includeErr := includeFile(path)
			if includeErr != nil {
				return nil, includeErr
			}
			if file != nil {
				result["summaries"] = map[string]any{"file": file, "text": lastSummaries(text, 3)}
			}
		}
	}
	return result, nil
}

func firstQuery(q map[string][]string, name string) string {
	if values := q[name]; len(values) > 0 {
		return values[0]
	}
	return ""
}

func (e *Engine) handleAgentTranscript(w http.ResponseWriter, r *http.Request, a *agentCall) {
	node := a.resolve(r.PathValue("node"))
	if !a.canObserveTranscript(node) && !allows(a.scope, ScopeAdmin) {
		writeErr(w, errForbidden("you may read only yourself, your children, or nodes joined to you"))
		return
	}
	result, err := a.c.transcript(node, r.URL.Query(), a.node.ID)
	if err != nil {
		writeErr(w, err)
		return
	}
	writeJSON(w, http.StatusOK, result)
}

func (a *agentCall) canObserveTranscript(target string) bool {
	if a.c.node(target) == nil {
		return false
	}
	return a.c.canObserveTranscript(a.node.ID, target)
}

func (e *Engine) handleAgentSearch(w http.ResponseWriter, r *http.Request, a *agentCall) {
	q := r.URL.Query()
	refs := []string{r.PathValue("node")}
	if strings.EqualFold(strings.TrimSpace(q.Get("nodes")), "children") {
		refs = a.c.children(a.node.ID)
		if len(refs) == 0 {
			writeErr(w, errInvalid("you have no children to search"))
			return
		}
	}
	query := strings.TrimSpace(q.Get("q"))
	if query == "" {
		writeErr(w, errInvalid("q is required"))
		return
	}
	limit := 0
	if value := strings.TrimSpace(q.Get("limit")); value != "" {
		var err error
		limit, err = strconv.Atoi(value)
		if err != nil || limit < 1 {
			writeErr(w, errInvalid("limit is a positive integer"))
			return
		}
	}
	results := make([]map[string]any, 0, len(refs))
	for _, ref := range refs {
		node := a.resolve(ref)
		if !a.canObserveTranscript(node) && !allows(a.scope, ScopeAdmin) {
			writeErr(w, errForbidden("you may search only yourself, your children, or nodes joined to you"))
			return
		}
		path, err := a.c.transcriptSession(node)
		if err != nil {
			if errors.Is(err, fs.ErrNotExist) {
				continue
			}
			writeErr(w, err)
			return
		}
		text, err := transcript.Search(path, query, limit)
		if err != nil {
			writeErr(w, errInvalid(err.Error()))
			return
		}
		results = append(results, map[string]any{
			"node": node, "session": filepath.Base(strings.TrimSuffix(path, ".session.jsonl")), "text": text,
		})
	}
	if len(refs) == 1 {
		if len(results) == 0 {
			writeErr(w, errNotFound("the node has no session transcript"))
			return
		}
		writeJSON(w, http.StatusOK, results[0])
		return
	}
	writeJSON(w, http.StatusOK, map[string]any{"query": query, "results": results})
}
