package main

import (
	"encoding/json/v2"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// benchSurface is a version of a surface as the genui plugin writes it:
// metrics, a table and a chart of a file, a page, a badge, a field and a
// button, with what the file held when it was made.
func benchSurface(version int) string {
	return strings.ReplaceAll(`{"kou":"genui/v0","surface":"bench","version":VERSION,"session":"s1","title":"Bench","glance":"p99 {#p99}",
"sources":{"runs":{"file":"runs.jsonl","format":"jsonl"}},
"components":[
 {"id":"root","component":"Column","children":["kpis","table","trend","page","state","comment","apply"]},
 {"id":"kpis","component":"Row","children":["p50","p99"]},
 {"id":"p50","component":"Metric","label":"p50","unit":"ms","digits":1,"better":"lower","value":{"path":"/sources/runs/last/p50"}},
 {"id":"p99","component":"Metric","label":"p99","unit":"ms","digits":1,"better":"lower","value":{"path":"/sources/runs/last/p99"}},
 {"id":"table","component":"Table","rows":{"path":"/sources/runs/rows"},"columns":["commit","p50","p99"]},
 {"id":"trend","component":"Chart","kind":"line","data":{"path":"/sources/runs/rows"},"x":"commit","y":"p99","unit":"ms","alt":"p99 by commit"},
 {"id":"page","component":"Html","file":"tools/deps.html","alt":"the modules by owner"},
 {"id":"state","component":"Badge","text":"running","tone":"ok"},
 {"id":"comment","component":"TextField","label":"Comment","placeholder":"say","value":{"path":"/comment"}},
 {"id":"apply","component":"Button","label":"Apply","style":"primary","once":true,
  "action":{"event":{"name":"apply","data":{"comment":{"path":"/comment"}}}}}],
"data":{},
"snapshot":{"sources":{"runs":{"rows":[{"commit":"a1","p50":10,"p99":40},{"commit":"b2","p50":11,"p99":41.6}],"last":{"commit":"b2","p50":11,"p99":41.6},"count":2}},
 "sourceInfo":{"runs":{"path":"runs.jsonl","format":"jsonl","size":80,"mtime":"2026-10-21T10:00:00Z"}}}}`, "VERSION", strconv.Itoa(version))
}

// benchOut is what the agent got of the call that made a version.
func benchOut(version int) string {
	v := strconv.Itoa(version)
	return "UI bench v" + v + " (.harness/ui/s1/bench/v" + v + ".json)\n\n◇ Bench"
}

// applyUI puts a finished call of the UI tool in the transcript.
func applyUI(t *testing.T, m *uiModel, seq int, callID, out string) {
	t.Helper()
	state, err := json.Marshal(operation.ShellState{Result: &operation.ShellResult{Out: out}})
	if err != nil {
		t.Fatal(err)
	}
	op := operation.ID("o" + callID)
	for _, line := range []string{
		item(t, seq, sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "t", Response: llm.Response{Output: []llm.Item{
			{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: callID, Name: "UI", Arguments: `{"surface":"bench"}`}}}}}),
		item(t, seq+1, sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{TurnID: "t", CallID: callID,
			Status: tool.CallStatus{WaitingFor: []operation.ID{op}},
			Operations: []operation.Operation{{ID: op, Type: operation.TypeShell, Version: operation.VersionShell,
				Status: operation.StatusCompleted, State: state}}}),
	} {
		if _, err := m.tr.Apply([]byte(line)); err != nil {
			t.Fatal(err)
		}
	}
}

// readSurfaces looks at the live surfaces' files, as the tick does.
func readSurfaces(t *testing.T, m *uiModel) {
	t.Helper()
	cmd := m.surfacesUpdate(surfaceTickMsg{})
	if cmd == nil {
		t.Fatal("no look at the files")
	}
	msg, ok := cmd().(surfaceReadMsg)
	if !ok {
		t.Fatal("the look is not a read")
	}
	m.Update(msg)
}

// entryLines are the transcript's lines of an entry, as plain text.
func entryLines(t *testing.T, m *uiModel, id string) []string {
	t.Helper()
	for _, s := range m.spans {
		if s.id == id {
			return plain(m.lines[s.start:s.end])
		}
	}
	t.Fatalf("no entry %s in the transcript", id)
	return nil
}

func benchModel(t *testing.T) *uiModel {
	t.Helper()
	m := testModel(t)
	ws := m.opt.Workspace
	writeFile(t, ws, ".harness/ui/s1/bench/v1.json", benchSurface(1))
	writeFile(t, ws, "runs.jsonl", `{"commit":"a1","p50":10,"p99":40}
{"commit":"b2","p50":11,"p99":41.6}
{"commit":"c3","p50":12,"p99":50}
`)
	m.tr.Submit("7c5d3f8e-0f7a-4d38-9d8f-2f1c3a7e9b10", "show", time.Now())
	applyUI(t, m, 1, "c1", benchOut(1))
	readSurfaces(t, m)
	return m
}

func TestSurfaceDrawsWhatItShowed(t *testing.T) {
	dir := t.TempDir()
	writeFile(t, dir, ".harness/ui/s1/bench/v1.json", benchSurface(1))
	m := &uiModel{styles: newStyles(true)}
	m.opt.Workspace = dir
	for _, name := range []string{"UI", "onion_UI", "mcp__assault_kidney__onion_UI"} {
		e := &cockpit.Entry{ID: "tool:c1", Kind: cockpit.KindTool, At: time.Now(),
			Tool: &cockpit.Tool{Name: name, State: cockpit.ToolDone, Output: benchOut(1)}}
		drawn := m.renderTool(e, 100, true, false)
		for i, line := range drawn {
			if w := ansi.StringWidth(line); w > 100 {
				t.Fatalf("%s: line %d is %d wide: %q", name, i, w, ansi.Strip(line))
			}
		}
		text := strings.Join(plain(drawn), "\n")
		if name == "UI" {
			t.Log("\n" + text)
		}
		// Not the newest call in a transcript: what the files held then.
		for _, want := range []string{"UI  Bench v1 · p99 41.6 ms", "snapshot", "41.6 ms", "▲", "P99", "COMMIT", "b2",
			"p99 by commit", "HTML", "the modules by owner", "tools/deps.html · drawn live in the web cockpit and the iPhone app",
			"RUNNING", "Comment", "say", "Apply", "runs · 2 rows", "bench v1"} {
			if !strings.Contains(text, want) {
				t.Errorf("%s: no %q in\n%s", name, want, text)
			}
		}
		if strings.Contains(text, "ctrl+]") {
			t.Errorf("%s: a snapshot offers its controls:\n%s", name, text)
		}
		if closed := m.renderTool(e, 100, false, false); len(closed) != 1 {
			t.Errorf("%s: closed, the block is %d lines", name, len(closed))
		}
	}

	// Any other call draws as it did.
	bash := &cockpit.Entry{ID: "tool:c2", Kind: cockpit.KindTool, At: time.Now(),
		Tool: &cockpit.Tool{Name: "Bash", State: cockpit.ToolDone, Output: benchOut(1)}}
	if _, ok := m.surfaceTool(bash, 100, true); ok {
		t.Error("a Bash call drawn as a surface")
	}
	broken := &cockpit.Entry{ID: "tool:c3", Kind: cockpit.KindTool, At: time.Now(),
		Tool: &cockpit.Tool{Name: "UI", State: cockpit.ToolFailed, Output: "UI bench: nothing shown\n- components: root is missing"}}
	if _, ok := m.surfaceTool(broken, 100, true); ok {
		t.Error("a call that showed nothing drawn as a surface")
	}
}

func TestSurfaceFollowsItsFiles(t *testing.T) {
	m := benchModel(t)
	text := strings.Join(entryLines(t, m, "tool:c1"), "\n")
	t.Log("\n" + text)
	for _, want := range []string{"● live", "p99 50.0 ms", "50.0 ms", "c3", "runs · 3 rows", "ctrl+] focus"} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}

	// A row more: the next look shows it, without another call.
	f, err := os.OpenFile(filepath.Join(m.opt.Workspace, "runs.jsonl"), os.O_APPEND|os.O_WRONLY, 0)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := f.WriteString(`{"commit":"d4","p50":13,"p99":60.25}` + "\n"); err != nil {
		t.Fatal(err)
	}
	f.Close()
	readSurfaces(t, m)
	text = strings.Join(entryLines(t, m, "tool:c1"), "\n")
	for _, want := range []string{"60.3 ms", "d4", "runs · 4 rows"} {
		if !strings.Contains(text, want) {
			t.Errorf("after a row more: no %q in\n%s", want, text)
		}
	}

	// Another call of the surface is the live one; the first shows what it
	// showed, folded.
	writeFile(t, m.opt.Workspace, ".harness/ui/s1/bench/v2.json", benchSurface(2))
	applyUI(t, m, 3, "c2", benchOut(2))
	readSurfaces(t, m)
	first, second := entryLines(t, m, "tool:c1"), entryLines(t, m, "tool:c2")
	if len(first) != 1 || !strings.Contains(first[0], "snapshot") || !strings.Contains(first[0], "v1") {
		t.Errorf("the first call:\n%s", strings.Join(first, "\n"))
	}
	if len(second) < 5 || !strings.Contains(second[0], "● live") || !strings.Contains(strings.Join(second, "\n"), "runs · 4 rows") {
		t.Errorf("the second call:\n%s", strings.Join(second, "\n"))
	}
}

func TestSurfaceControlsReachTheAgent(t *testing.T) {
	m := benchModel(t)
	m.state = running // a run whose output does not matter: events wait in the queue
	press(m, tea.KeyMsg{Type: tea.KeyCtrlCloseBracket})
	if f := m.surfaces.focus; f == nil || f.entry != "tool:c1" || f.control != "comment" {
		t.Fatalf("focus %+v", m.surfaces.focus)
	}
	if m.input.Focused() {
		t.Error("the composer kept the keys")
	}
	if footer := ansi.Strip(m.footer()); !strings.Contains(footer, "esc") || !strings.Contains(footer, "to the prompt") {
		t.Errorf("footer %q", footer)
	}
	press(m, runes("hi"))
	if m.input.Value() != "" {
		t.Fatalf("the composer got %q", m.input.Value())
	}
	if text := strings.Join(entryLines(t, m, "tool:c1"), "\n"); !strings.Contains(text, "hi") {
		t.Errorf("the field does not show what was typed:\n%s", text)
	}
	press(m, key("tab"))
	if f := m.surfaces.focus; f == nil || f.control != "apply" {
		t.Fatalf("after tab: focus %+v", m.surfaces.focus)
	}
	press(m, key("enter"))
	q := m.queue()
	if len(q.Items) != 1 || q.Items[0].Text != `[UI bench v1] apply {"comment":"hi"}` {
		t.Fatalf("queue %+v", q.Items)
	}
	// A once button is done with.
	press(m, key("enter"))
	if len(q.Items) != 1 {
		t.Fatalf("pressed twice: queue %+v", q.Items)
	}
	press(m, key("esc"))
	if m.surfaces.focus != nil || !m.input.Focused() {
		t.Fatalf("after esc: focus %+v, composer focused %v", m.surfaces.focus, m.input.Focused())
	}
}

func TestSurfaceClicks(t *testing.T) {
	m := benchModel(t)
	m.state = running
	// at finds a word of the surface's card on the screen.
	at := func(word string) (int, int) {
		t.Helper()
		for _, s := range m.spans {
			if s.id != "tool:c1" {
				continue
			}
			for i := s.start; i < s.end; i++ {
				line := ansi.Strip(m.lines[i])
				if col := strings.Index(line, word); col >= 0 {
					m.view.SetYOffset(max(0, i-m.view.Height/2))
					return ansi.StringWidth(line[:col]) + m.left(), i - m.view.YOffset + m.top()
				}
			}
		}
		t.Fatalf("no %q on the screen", word)
		return 0, 0
	}
	x, y := at("say")
	click(m, x, y)
	if f := m.surfaces.focus; f == nil || f.control != "comment" {
		t.Fatalf("a click on the field: focus %+v", m.surfaces.focus)
	}
	press(m, runes("ok"))
	x, y = at("Apply")
	click(m, x, y)
	if q := m.queue(); len(q.Items) != 1 || q.Items[0].Text != `[UI bench v1] apply {"comment":"ok"}` {
		t.Fatalf("a click on the button: queue %+v", q.Items)
	}
	// A click elsewhere gives the keys back.
	click(m, 0, m.top())
	if m.surfaces.focus != nil {
		t.Fatalf("a click elsewhere: focus %+v", m.surfaces.focus)
	}
}

// everySurface has a component of every kind the terminal draws.
const everySurface = `{"kou":"genui/v0","surface":"all","version":1,"session":"s1","title":"All","sources":{},
"components":[
 {"id":"root","component":"Column","children":["sec","tabs","more","grid","list","progress","code","diff","link","image","multi","unknown","controls"]},
 {"id":"sec","component":"Section","title":"Overview","children":["heading","md"]},
 {"id":"heading","component":"Text","text":"A heading","style":"heading"},
 {"id":"md","component":"Markdown","text":"Some **bold** text and a list:\n\n- one\n- two"},
 {"id":"tabs","component":"Tabs","tabs":[{"label":"First","child":"t1"},{"label":"Second","child":"t2"}]},
 {"id":"t1","component":"Text","text":"in the first tab"},
 {"id":"t2","component":"Text","text":"in the second tab"},
 {"id":"more","component":"Disclosure","label":"Details","children":["hidden"]},
 {"id":"hidden","component":"Text","text":"the hidden text"},
 {"id":"grid","component":"Grid","columns":2,"children":["b1","b2","b3"]},
 {"id":"b1","component":"Badge","text":"fine","tone":"ok"},
 {"id":"b2","component":"Badge","text":"slow","tone":"warn"},
 {"id":"b3","component":"Badge","text":"down","tone":"err"},
 {"id":"list","component":"List","items":[{"name":"api","status":"up"},{"name":"db","status":"down"}],"text":"{/name}: {/status}"},
 {"id":"progress","component":"Progress","label":"Build","value":3,"max":4},
 {"id":"code","component":"Code","text":"func main() {\n\tprintln(\"hi\")\n}","language":"go"},
 {"id":"diff","component":"Diff","patch":"--- a/x.txt\n+++ b/x.txt\n@@ -1 +1 @@\n-old line\n+new line\n"},
 {"id":"link","component":"FileLink","file":"go.mod","line":3,"label":"the module"},
 {"id":"image","component":"Image","file":"shot.png","alt":"a screenshot"},
 {"id":"multi","component":"Chart","kind":"line","data":[{"x":1,"a":1,"b":5},{"x":2,"a":2,"b":4},{"x":3,"a":3,"b":3}],"x":"x","y":["a","b"],"alt":"a rises, b falls"},
 {"id":"unknown","component":"Globe","alt":"a globe"},
 {"id":"controls","component":"Row","children":["toggle","select","slider","danger"]},
 {"id":"toggle","component":"Toggle","label":"Verbose","value":{"path":"/verbose"}},
 {"id":"select","component":"Select","label":"Mode","value":{"path":"/mode"},"options":["fast","safe"]},
 {"id":"slider","component":"Slider","label":"Level","value":{"path":"/level"},"min":0,"max":10,"step":1},
 {"id":"danger","component":"Button","label":"Verbose on","style":"danger","action":{"set":{"/verbose":true}}}],
"data":{"mode":"safe","level":4},"snapshot":{"sources":{},"sourceInfo":{}}}`

func TestSurfaceDrawsEveryComponent(t *testing.T) {
	m := testModel(t)
	writeFile(t, m.opt.Workspace, ".harness/ui/s1/all/v1.json", everySurface)
	m.tr.Submit("7c5d3f8e-0f7a-4d38-9d8f-2f1c3a7e9b10", "show", time.Now())
	applyUI(t, m, 1, "c1", "UI all v1 (.harness/ui/s1/all/v1.json)\n\n◇ All")
	m.surfacesUpdate(nil)
	show := func() string {
		t.Helper()
		lines := entryLines(t, m, "tool:c1")
		for i, line := range m.lines {
			if w := ansi.StringWidth(line); w > m.view.Width {
				t.Fatalf("line %d is %d wide, over %d: %q", i, w, m.view.Width, ansi.Strip(line))
			}
		}
		return strings.Join(lines, "\n")
	}
	text := show()
	t.Log("\n" + text)
	for _, want := range []string{"OVERVIEW", "A heading", "bold", "• one", "[First]  Second", "in the first tab", "▸ Details",
		"FINE", "SLOW", "DOWN", "· api: up", "Build", "75%", "println", "-old line", "+new line", "↗ the module",
		"a screenshot", "a rises, b falls", "a globe", "[ ] Verbose", "Mode ‹ safe ›", "Level", "─ 4"} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q in\n%s", want, text)
		}
	}
	for _, not := range []string{"in the second tab", "the hidden text"} {
		if strings.Contains(text, not) {
			t.Errorf("%q shows:\n%s", not, text)
		}
	}

	// The keys go through the controls in the order they show.
	press(m, tea.KeyMsg{Type: tea.KeyCtrlCloseBracket})
	var order []string
	for range 8 {
		if f := m.surfaces.focus; f != nil {
			order = append(order, f.control)
		}
		press(m, key("tab"))
	}
	if got := strings.Join(order, " "); got != "tabs more link toggle select slider danger tabs" {
		t.Fatalf("order %s", got)
	}
	// The loop left the focus on the disclosure: back to the tabs, where →
	// shows the second.
	press(m, key("shift+tab"))
	press(m, key("right"))
	press(m, key("tab"))
	press(m, key("enter")) // opens the disclosure
	press(m, key("right")) // not a value: → moves on, to the link
	press(m, key("tab"))
	press(m, runes(" ")) // the toggle
	press(m, key("tab"))
	press(m, key("right")) // safe → fast, round
	press(m, key("tab"))
	press(m, key("right")) // 4 → 5
	press(m, key("tab"))
	press(m, key("enter")) // sets /verbose, as it is already
	text = show()
	for _, want := range []string{"in the second tab", "the hidden text", "[x] Verbose", "‹ fast ›", "─ 5"} {
		if !strings.Contains(text, want) {
			t.Errorf("no %q after the keys in\n%s", want, text)
		}
	}
	for _, not := range []string{"in the first tab", "safe"} {
		if strings.Contains(text, not) {
			t.Errorf("%q shows after the keys:\n%s", not, text)
		}
	}
	doc := m.surfaces.edits[".harness/ui/s1/all/v1.json"]
	t.Logf("edits %#v", doc)
	if len(m.queue().Items) != 0 {
		t.Errorf("a set button reached the agent: %+v", m.queue().Items)
	}
	t.Log("\n" + text)

	// Narrow, everything still fits.
	m.Update(tea.WindowSizeMsg{Width: 44, Height: 30})
	t.Log("\n" + show())
}

func TestNoSurfaceLeavesTheKey(t *testing.T) {
	m := testModel(t)
	if _, ok := m.surfaceKey(tea.KeyMsg{Type: tea.KeyCtrlCloseBracket}); ok {
		t.Error("ctrl+] taken with no surface")
	}
	if items := m.surfacePalette(); len(items) != 0 {
		t.Errorf("palette %+v", items)
	}
}
