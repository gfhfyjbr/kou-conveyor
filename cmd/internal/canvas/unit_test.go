package canvas

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

func TestPlaceGoesBesideWithoutCoveringAndIsTheSameEachTime(t *testing.T) {
	if got := placeNear(nil, nil, "", 760, 460); got != (Rect{0, 0, 760, 460}) {
		t.Fatalf("the first node goes at the origin: %+v", got)
	}
	first := Rect{0, 0, 760, 460}
	if got := placeNear([]Rect{first}, nil, "", 440, 560); got != (Rect{800, 0, 440, 560}) {
		t.Fatalf("a node near none goes right of them all, gap apart: %+v", got)
	}
	if got := placeNear([]Rect{first}, &first, "below", 280, 180); got != (Rect{0, 504, 280, 180}) {
		t.Fatalf("below: %+v", got)
	}
	if got := placeNear([]Rect{first}, &first, "left", 280, 180); got != (Rect{-320, 0, 280, 180}) {
		t.Fatalf("left: %+v", got)
	}

	place := func() []Rect {
		taken := []Rect{first}
		for range 24 {
			taken = append(taken, placeNear(taken, &first, "right", 320, 220))
		}
		return taken
	}
	one, two := place(), place()
	if !slices.Equal(one, two) {
		t.Fatal("the same canvas gave different places")
	}
	for i, a := range one {
		if a.X%gridSize != 0 || a.Y%gridSize != 0 {
			t.Errorf("%+v is off the grid", a)
		}
		for _, b := range one[i+1:] {
			if overlaps(a, b, 0) {
				t.Fatalf("%+v covers %+v", a, b)
			}
		}
	}

	// A place given that covers a node moves down until it is free.
	shifted := shiftFree([]Rect{first}, Rect{100, 100, 200, 100})
	if overlaps(shifted, first, placeMargin) || shifted.X != 100 || shifted.Y <= 100 {
		t.Fatalf("shifted to %+v", shifted)
	}
	if free := (Rect{2000, 0, 200, 100}); shiftFree([]Rect{first}, free) != free {
		t.Fatal("a free place moved")
	}
}

func TestTokensSayTheirNodeAndCannotBeForged(t *testing.T) {
	secret := []byte("0123456789abcdef0123456789abcdef")
	want := claims{Canvas: "3f1c-canvas", Node: "n_abc123", Epoch: 2, Scope: ScopeTalk}
	token := mintToken(secret, want)
	got, err := parseToken(secret, token)
	if err != nil || got != want {
		t.Fatalf("parse(%q) = %+v, %v", token, got, err)
	}
	parts := strings.Split(token, ".")
	forged := strings.Join([]string{parts[0], parts[1], parts[2], parts[3], ScopeAdmin, parts[5]}, ".")
	for _, bad := range []string{
		forged,
		strings.Replace(token, "n_abc123", "n_abc124", 1),
		"kc2" + strings.TrimPrefix(token, "kc1"),
		token + ".x",
		"",
	} {
		if _, err := parseToken(secret, bad); err == nil {
			t.Errorf("%q passed", bad)
		}
	}
	if _, err := parseToken([]byte("another secret, of another server"), token); err == nil {
		t.Error("a token of another secret passed")
	}
	if _, err := parseToken(secret, mintToken(secret, claims{Canvas: "c", Node: "n_abc123", Scope: "root"})); err == nil {
		t.Error("a token of no scope passed")
	}
}

func TestScopesAllowWhatTheOnesBeforeDo(t *testing.T) {
	for _, tc := range []struct {
		scope, need string
		want        bool
	}{
		{ScopeAdmin, ScopeBuild, true}, {ScopeBuild, ScopeTalk, true}, {ScopeTalk, ScopeObserve, true},
		{ScopeTalk, ScopeBuild, false}, {ScopeObserve, ScopeTalk, false}, {ScopeNone, ScopeObserve, false},
		{"", ScopeObserve, false}, {scopeSource, ScopeObserve, false},
	} {
		if got := allows(tc.scope, tc.need); got != tc.want {
			t.Errorf("allows(%q, %q) = %v", tc.scope, tc.need, got)
		}
	}
}

func TestTemplatesMakeMessages(t *testing.T) {
	from := &Node{ID: "n_aaaaaa", Kind: KindTerminal, Preset: "shell", Title: "Builder"}
	vars := messageVars{
		text: "hi", title: "echo hi", from: from, edge: "e_bbbbbb", now: time.Date(2026, 10, 2, 12, 0, 0, 0, time.UTC),
		data: jsontext.Value(`{"exit_code":0,"issue":{"number":42,"labels":["bug","ui"]},"name":"x"}`),
	}
	for template, want := range map[string]string{
		"":                       "hi",
		"   ":                    "hi",
		`echo "got: {{text}}"`:   `echo "got: hi"`,
		"{{ title }} → {{text}}": "echo hi → hi",
		"#{{data.issue.number}} {{data.issue.labels.1}} {{data.exit_code}}": "#42 ui 0",
		"{{data.issue.labels}}":                         `["bug","ui"]`,
		"[{{missing}}][{{data.nope}}][{{data.name.x}}]": "[][][]",
		"{{from.title}} {{from.id}} {{from.kind}}":      "Builder n_aaaaaa shell",
		"{{edge.id}} {{now}}":                           "e_bbbbbb 2026-10-02T12:00:00Z",
		"{{text":                                        "{{text",
	} {
		if got := renderMessage(template, vars, false); got != want {
			t.Errorf("%q made %q, want %q", template, got, want)
		}
	}
	if got := renderMessage("", vars, true); got != "[canvas] from «Builder» (n_aaaaaa):\nhi" {
		t.Errorf("with the header: %q", got)
	}
}

func TestArgumentsWithoutTheirValuesGoWithTheirFlags(t *testing.T) {
	values := map[string]string{"runtime.agent_session": "s-1", "config.model": ""}
	lookup := func(name string) (string, bool) {
		value, ok := values[name]
		return value, ok && value != ""
	}
	got := expandArgs([]string{
		"--session-id", "{{runtime.agent_session}}",
		"--model", "{{config.model}}",
		"plain", "{{missing}}",
	}, lookup)
	want := []string{"--session-id", "s-1", "plain"}
	if !slices.Equal(got, want) {
		t.Fatalf("got %q, want %q", got, want)
	}
}

func TestDocumentsReadFromDiskAreMadeWhole(t *testing.T) {
	workspace := t.TempDir()
	path := docPath(workspace, "board-1")
	data := `{
		"version": 1, "id": "elsewhere", "title": "Board", "live": true, "future": {"kept": true},
		"settings": {"autonomy": {"spawn": "ask"}, "routing": {"max_hops": 4}},
		"nodes": [
			{"id": "n_aaaaaa", "kind": "note", "title": "A", "x": 1, "y": 2, "w": 10, "h": 99999, "color": "red"},
			{"id": "n_aaaaaa", "kind": "note", "title": "A again"},
			{"id": "bad", "kind": "note", "title": "no ID of a node"},
			{"id": "n_bbbbbb", "kind": "robot", "title": "of no kind"},
			{"id": "n_cccccc", "kind": "agent", "title": "C", "w": 500, "h": 600}
		],
		"edges": [
			{"id": "e_aaaaaa", "from": {"node": "n_cccccc", "port": "out"}, "to": {"node": "n_aaaaaa", "port": "in"}},
			{"id": "e_bbbbbb", "from": {"node": "n_bbbbbb", "port": "out"}, "to": {"node": "n_cccccc", "port": "in"}},
			{"id": "nope", "from": {"node": "n_cccccc", "port": "out"}, "to": {"node": "n_aaaaaa", "port": "in"}}
		]
	}`
	if err := writeAtomic(path, []byte(data), 0o600); err != nil {
		t.Fatal(err)
	}
	doc, readOnly, err := loadDoc(workspace, "board-1")
	if err != nil || readOnly {
		t.Fatalf("load: %v, read-only %v", err, readOnly)
	}
	if doc.ID != "board-1" {
		t.Errorf("the ID is the file's: %q", doc.ID)
	}
	var ids []string
	for _, n := range doc.Nodes {
		ids = append(ids, n.ID)
	}
	if !slices.Equal(ids, []string{"n_aaaaaa", "n_cccccc"}) {
		t.Fatalf("nodes %v", ids)
	}
	if a := doc.Nodes[0]; a.W != 280 || a.H != 180 || a.X != 1 || a.Y != 2 {
		t.Errorf("sizes out of bounds are the default, places kept: %+v", a.rect())
	}
	if c := doc.Nodes[1]; c.W != 500 || c.H != 600 {
		t.Errorf("sizes in bounds are kept: %+v", c.rect())
	}
	if len(doc.Edges) != 1 || doc.Edges[0].ID != "e_aaaaaa" {
		t.Fatalf("edges %+v", doc.Edges)
	}
	s := doc.Settings
	if s.Autonomy.Spawn != "ask" || s.Routing.MaxHops != 4 || s.Autonomy.MaxNodes != 48 || s.Routing.EdgeRatePerMinute != 30 {
		t.Errorf("settings %+v", s)
	}
	// What a newer version wrote goes back as it was.
	if err := saveDoc(workspace, doc); err != nil {
		t.Fatal(err)
	}
	written, _ := os.ReadFile(path)
	if !strings.Contains(string(written), `"future"`) || !strings.Contains(string(written), `"color": "red"`) {
		t.Errorf("unknown fields were lost:\n%s", written)
	}

	newer := strings.Replace(data, `"version": 1`, `"version": 7`, 1)
	if err := writeAtomic(docPath(workspace, "board-2"), []byte(newer), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, readOnly, err := loadDoc(workspace, "board-2"); err != nil || !readOnly {
		t.Errorf("a newer version's canvas is read-only: %v, %v", readOnly, err)
	}
	if ids := listIDs(workspace); !slices.Equal(ids, []string{"board-1", "board-2"}) {
		t.Errorf("listIDs = %v", ids)
	}
	if _, _, err := loadDoc(workspace, "../escape"); err == nil {
		t.Error("an ID that is not one was read")
	}
}

func TestSettingsAreChecked(t *testing.T) {
	if err := DefaultSettings().validate(); err != nil {
		t.Fatal(err)
	}
	bad := DefaultSettings()
	bad.Autonomy.Spawn = "maybe"
	if bad.validate() == nil {
		t.Error("spawn maybe passed")
	}
	bad = DefaultSettings()
	bad.Autonomy.MaxTerminals = 65
	if bad.validate() == nil {
		t.Error("65 terminals passed")
	}
	bad = DefaultSettings()
	bad.Routing.MaxHops = 0
	if bad.validate() == nil {
		t.Error("no hops passed")
	}
}

func TestConfigChangesMerge(t *testing.T) {
	merged, err := mergeConfig(jsontext.Value(`{"a":1,"b":{"c":2}}`), jsontext.Value(`{"b":null,"d":"x"}`))
	if err != nil || string(merged) != `{"a":1,"d":"x"}` {
		t.Fatalf("merged %s, %v", merged, err)
	}
	if merged, err := mergeConfig(jsontext.Value(`{"a":1}`), jsontext.Value(`{"a":null}`)); err != nil || merged != nil {
		t.Fatalf("all gone: %s, %v", merged, err)
	}
	if _, err := mergeConfig(nil, jsontext.Value(`[1]`)); err == nil {
		t.Fatal("an array merged")
	}
}

func TestTitlesAreOneShortLine(t *testing.T) {
	if got := cleanTitle(" a\nb\x01c\u0085d\t e "); got != "a bcd e" {
		t.Errorf("got %q", got)
	}
	long := cleanTitle(strings.Repeat("я", 300))
	if n := len([]rune(long)); n != 120 || !strings.HasSuffix(long, "…") {
		t.Errorf("%d runes", n)
	}
	if got := cut("aяb", 2); got != "a" {
		t.Errorf("cut in a rune: %q", got)
	}
}

func TestGlobsMatchAcrossDirectories(t *testing.T) {
	for _, tc := range []struct {
		pattern, name string
		want          bool
	}{
		{"**/*", "a/b/c.go", true},
		{"**/*.go", "c.go", true},
		{"**/*.go", "a/b/c.go", true},
		{"*.go", "a/c.go", false},
		{"src/**", "src/a/b", true},
		{"src/**/x.txt", "src/x.txt", true},
		{"src/**/x.txt", "lib/x.txt", false},
		{"docs/*.md", "docs/a.md", true},
	} {
		if got := globMatch(tc.pattern, tc.name); got != tc.want {
			t.Errorf("globMatch(%q, %q) = %v", tc.pattern, tc.name, got)
		}
	}
}

func TestTimeOfDayComesNext(t *testing.T) {
	from := time.Date(2026, 10, 2, 10, 0, 0, 0, time.UTC)
	if next, err := nextTimeOfDay("09:30", from); err != nil || !next.Equal(time.Date(2026, 10, 3, 9, 30, 0, 0, time.UTC)) {
		t.Errorf("09:30 → %v, %v", next, err)
	}
	if next, err := nextTimeOfDay("10:15", from); err != nil || !next.Equal(time.Date(2026, 10, 2, 10, 15, 0, 0, time.UTC)) {
		t.Errorf("10:15 → %v, %v", next, err)
	}
	if _, err := nextTimeOfDay("25:00", from); err == nil {
		t.Error("25:00 passed")
	}
}

func TestHubGivesWhatAPageMissed(t *testing.T) {
	h := newHub("run1")
	for i := range 3 {
		h.publish(map[string]any{"n": i})
	}
	events, ok := h.since(1)
	if !ok || len(events) != 2 || events[0].seq != 2 {
		t.Fatalf("since 1: %v %v", events, ok)
	}
	if events, ok := h.since(3); !ok || len(events) != 0 {
		t.Fatalf("since 3: %v %v", events, ok)
	}
	if _, ok := h.since(9); ok {
		t.Fatal("since the future")
	}
	if seq, ok := h.parseEventID(h.eventID(2)); !ok || seq != 2 {
		t.Fatalf("event ID: %d %v", seq, ok)
	}
	if _, ok := h.parseEventID("run0-2"); ok {
		t.Fatal("another run's ID")
	}
	for i := range hubKeep + 10 {
		h.publish(map[string]any{"n": i})
	}
	if _, ok := h.since(1); ok {
		t.Fatal("events no longer kept were given")
	}
}

func TestMessagesAreJournaledAndThoseThatWaitKept(t *testing.T) {
	dir := t.TempDir()
	one := &Message{ID: "m_1", Text: "one", State: MessageDelivered}
	two := &Message{ID: "m_2", Text: "two", State: MessageDropped}
	if err := appendJournal(dir, one, two, &Message{ID: "m_1", Text: "one", State: MessageDropped}); err != nil {
		t.Fatal(err)
	}
	got := readJournal(dir, 10)
	if len(got) != 2 || got[0].ID != "m_1" || got[0].State != MessageDropped || got[1].ID != "m_2" {
		t.Fatalf("journal %+v", got)
	}
	if got := readJournal(dir, 1); len(got) != 1 || got[0].ID != "m_2" {
		t.Fatalf("the last one: %+v", got)
	}
	waiting := []*Message{
		{ID: "m_3", State: MessagePending},
		{ID: "m_4", State: MessageAwaiting},
		{ID: "m_5", State: messageDelivering},
		{ID: "m_6", State: MessageDelivered},
	}
	if err := savePending(dir, waiting); err != nil {
		t.Fatal(err)
	}
	loaded := loadPending(dir)
	var states []string
	for _, m := range loaded {
		states = append(states, m.ID+":"+m.State)
	}
	if !slices.Equal(states, []string{"m_3:pending", "m_4:awaiting_approval", "m_5:pending"}) {
		t.Fatalf("pending %v", states)
	}
	if err := savePending(dir, nil); err != nil {
		t.Fatal(err)
	}
	if _, err := os.Stat(filepath.Join(dir, "pending.jsonl")); !os.IsNotExist(err) {
		t.Fatal("no message waits, and the file stays")
	}
}

func TestTheSecretIsMadeOnceAndKept(t *testing.T) {
	path := filepath.Join(t.TempDir(), "conf", "canvas.secret")
	one, err := LoadSecret(path)
	if err != nil || len(one) != 32 {
		t.Fatalf("%x, %v", one, err)
	}
	two, err := LoadSecret(path)
	if err != nil || string(one) != string(two) {
		t.Fatal("a second secret")
	}
	if info, err := os.Stat(path); err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("mode %v, %v", info.Mode(), err)
	}
}

func TestTemplatesOfTheGalleryAreBatches(t *testing.T) {
	for _, tmpl := range gallery {
		ops := tmpl.ops(100, 50)
		nodes, edges := 0, 0
		for _, op := range ops {
			switch op.Op {
			case "node.add":
				nodes++
				if op.Node.X == nil || !op.Node.Exact {
					t.Errorf("%s: a node of a template goes where it says", tmpl.ID)
				}
			case "edge.add":
				edges++
				if !strings.HasPrefix(op.Edge.From.Node, "$") || !strings.HasPrefix(op.Edge.To.Node, "$") {
					t.Errorf("%s: an edge names what the batch adds", tmpl.ID)
				}
			}
		}
		if nodes != len(tmpl.Nodes) || edges != len(tmpl.Edges) {
			t.Errorf("%s: %d nodes and %d edges", tmpl.ID, nodes, edges)
		}
		data, err := json.Marshal(tmpl)
		if err != nil || !jsontext.Value(data).IsValid() {
			t.Errorf("%s: %v", tmpl.ID, err)
		}
	}
}

// A shell that does not mark where its commands start (bash 3.2, macOS's
// /bin/bash) still gives what its commands print.
func TestCommandsOfShellsWithoutStartMarks(t *testing.T) {
	command, text, ok := unmarkedCommand([]byte("echo hi; echo there\r\nhi\r\nthere\r\n"))
	if !ok || command != "echo hi; echo there" || text != "hi\nthere" {
		t.Fatalf("%q %q %v", command, text, ok)
	}
	if command, text, ok := unmarkedCommand([]byte("true\r\n")); !ok || command != "true" || text != "" {
		t.Fatalf("a command that prints nothing: %q %q %v", command, text, ok)
	}
	for _, line := range []string{"", "\r\n", "  \r\n", "sleep 10^C\r\n"} {
		if _, _, ok := unmarkedCommand([]byte(line)); ok {
			t.Errorf("%q ran nothing", line)
		}
	}
}

// A harness that answered is not at work again for what it showed while it
// answered; one whose hooks say when its turns start is at work again by
// them, save after a question it asked.
func TestAHarnessThatAnsweredIsNotBusyAgainForItsOwnAnswer(t *testing.T) {
	at := time.Now()
	harness := func(status ...string) *terminalInstance {
		return &terminalInstance{harnessUp: true, def: harnessDef{Harness: plugin.CanvasHarness{Status: status}}}
	}
	// It showed its answer from 3s ago until just now, and fell idle 1s ago.
	answered := func(ti *terminalInstance) *terminalInstance {
		ti.activeSince, ti.lastActive = at.Add(-3*time.Second), at.Add(-100*time.Millisecond)
		return ti
	}
	idle := Status{State: StateIdle, Since: at.Add(-time.Second)}
	waiting := Status{State: StateWaiting, Since: at.Add(-time.Second)}
	if answered(harness("hooks")).resumed(at, idle) {
		t.Error("a hooks harness is busy again for the answer it just gave")
	}
	if answered(harness("idle")).resumed(at, idle) {
		t.Error("a harness is busy again for the answer it just gave")
	}

	// It shows something new, that began after it fell idle.
	fresh := func(ti *terminalInstance) *terminalInstance {
		ti.activeSince, ti.lastActive = at.Add(-1800*time.Millisecond), at.Add(-100*time.Millisecond)
		return ti
	}
	idle.Since, waiting.Since = at.Add(-5*time.Second), at.Add(-5*time.Second)
	if fresh(harness("hooks")).resumed(at, idle) {
		t.Error("a hooks harness is busy again where no hook said so")
	}
	if !fresh(harness("hooks")).resumed(at, waiting) {
		t.Error("a hooks harness whose question was answered is not at work")
	}
	if !fresh(harness("idle")).resumed(at, idle) {
		t.Error("a harness the user typed to is not at work")
	}
}
