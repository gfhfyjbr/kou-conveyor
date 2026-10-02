package main

import (
	"fmt"
	"slices"
	"strings"
	"testing"
	"time"

	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/highlight"
)

// codeEntry is a Code call whose code read n files, one after another, a
// second each; the reads in failed failed.
func codeEntry(n int, failed ...int) *cockpit.Entry {
	start := time.Date(2026, 9, 29, 12, 0, 0, 0, time.UTC)
	tool := &cockpit.Tool{
		CallID: "c1", Name: "Code", Input: "for (const file of files) await readText(file);", State: cockpit.ToolDone,
		Started: start, Finished: start.Add(time.Duration(n) * time.Second),
	}
	for i := range n {
		call := cockpit.CodeCall{
			Name: "readText", Input: fmt.Sprintf("/work/file%02d.go", i), Gist: fmt.Sprintf("file%02d.go", i), Output: "package main",
			State: cockpit.ToolDone, Started: start.Add(time.Duration(i) * time.Second), Finished: start.Add(time.Duration(i+1) * time.Second),
		}
		if slices.Contains(failed, i) {
			call.State, call.Output, call.Error = cockpit.ToolFailed, "", fmt.Sprintf("readText: file%02d.go does not exist", i)
		}
		tool.Calls = append(tool.Calls, call)
	}
	return &cockpit.Entry{ID: "tool:c1", Kind: cockpit.KindTool, At: start, Tool: tool}
}

func plain(lines []string) []string {
	out := make([]string, len(lines))
	for i, line := range lines {
		out[i] = strings.TrimRight(ansi.Strip(line), " ")
	}
	return out
}

func TestCodeCallsHangFromTheCall(t *testing.T) {
	m := &uiModel{styles: newStyles(true)}
	lines := plain(m.renderTool(codeEntry(3, 1), 100, false, true))
	indent := strings.Repeat(" ", gutter-2)
	want := []string{
		indent + "├─ ✓ readText  file00.go",
		indent + "├─ ✗ readText  file01.go",
		indent + "│    readText: file01.go does not exist",
		indent + "└─ ✓ readText  file02.go",
	}
	if len(lines) != len(want)+1 {
		t.Fatalf("lines:\n%s", strings.Join(lines, "\n"))
	}
	for i, prefix := range want {
		line := lines[i+1]
		if !strings.HasPrefix(line, prefix) {
			t.Errorf("line %d = %q, want it to start %q", i+1, line, prefix)
		}
		// A call's lane shows when it ran within the run, and its time how long.
		if !strings.Contains(prefix, "does not exist") && (!strings.Contains(line, "━") || !strings.HasSuffix(line, "1s")) {
			t.Errorf("line %d = %q has no lane or time", i+1, line)
		}
	}
}

func TestCodeCallsFoldClosed(t *testing.T) {
	m := &uiModel{styles: newStyles(true)}
	entry := codeEntry(16, 6, 14)
	closed := strings.Join(plain(m.renderTool(entry, 100, false, true)), "\n")
	for _, want := range []string{"file03.go", "├─ ⋯ 8 more calls · 1 failed", "file12.go", "file14.go does not exist", "└─ ✓ readText  file15.go"} {
		if !strings.Contains(closed, want) {
			t.Errorf("closed tree lacks %q:\n%s", want, closed)
		}
	}
	if strings.Contains(closed, "file04.go") || strings.Contains(closed, "file11.go") {
		t.Errorf("closed tree shows the calls it folds:\n%s", closed)
	}
	open := strings.Join(plain(m.renderTool(entry, 100, true, true)), "\n")
	if !strings.Contains(open, "CALLS · 16") || strings.Contains(open, "more calls") || !strings.Contains(open, "file06.go does not exist") {
		t.Errorf("open tree:\n%s", open)
	}
	for i := range 16 {
		if name := fmt.Sprintf("file%02d.go", i); !strings.Contains(open, name) {
			t.Errorf("open tree lacks %s", name)
		}
	}
}

func TestCodeCallsOfAnInterruptedRun(t *testing.T) {
	m := &uiModel{styles: newStyles(true)}
	entry := codeEntry(2)
	entry.Tool.State = cockpit.ToolRunning
	running := &entry.Tool.Calls[1]
	running.State, running.Output, running.Finished = cockpit.ToolRunning, "", time.Time{}
	live := plain(m.renderTool(entry, 100, false, true))
	if last := live[len(live)-1]; !strings.Contains(last, "└─ ■ readText") || !strings.HasSuffix(last, "running") {
		t.Errorf("running call: %q", last)
	}
	// Nobody runs the session: the call was interrupted with its run.
	gone := plain(m.renderTool(entry, 100, false, false))
	if last := gone[len(gone)-1]; !strings.Contains(last, "└─ ◌ readText") || strings.Contains(last, "running") {
		t.Errorf("interrupted call: %q", last)
	}
}

func TestCodeCallsFitTheirWidth(t *testing.T) {
	m := &uiModel{styles: newStyles(false)}
	entry := codeEntry(16, 2, 13)
	entry.Tool.Calls[0].Input = "set -e\n" + strings.Repeat("echo wide ", 30)
	entry.Tool.Calls[0].Output = strings.Repeat("output ", 60)
	for _, width := range []int{24, 40, 80, 160} {
		for _, open := range []bool{false, true} {
			// Closed, the tree hangs under the call's line; open, it is a
			// section of the call's card, as wide as the card's body.
			lines, room := m.renderTool(entry, width, false, true), width
			if open {
				room = max(10, width-gutter)
				lines = m.renderCalls(entry.Tool, entry.Tool.State, "", room, true)
			}
			for i, line := range lines {
				if w := ansi.StringWidth(line); w > room {
					t.Errorf("width %d open %v: line %d is %d wide: %q", width, open, i, w, ansi.Strip(line))
				}
			}
			// Lanes only where there is room for them beside the calls.
			if lanes := strings.Contains(strings.Join(plain(lines), "\n"), "━"); lanes != (width >= 80) {
				t.Errorf("width %d open %v: lanes %v", width, open, lanes)
			}
		}
	}
}

// class is the index of a class of tokens in highlight.Classes.
func class(name string) int32 { return int32(slices.Index(highlight.Classes, name)) }

func TestCodeRows(t *testing.T) {
	show := func(rows [][]codePiece) string {
		var out []string
		for _, row := range rows {
			if row == nil {
				out = append(out, "⋯")
				continue
			}
			var parts []string
			for _, piece := range row {
				parts = append(parts, piece.class+":"+piece.text)
			}
			out = append(out, strings.Join(parts, "|"))
		}
		return strings.Join(out, "\n")
	}
	// A token a row ends in goes on in the next, in its colour; tabs are
	// four spaces, and an empty line a row of its own.
	code := "const s = 'abcdefghij';\n\n\tx\n"
	runs := []int32{5, class("k"), 5, 0, 12, class("s"), 1, class("p"), 4, 0}
	if got, want := show(codeRows(code, runs, 12)), "k:const|: s = |s:'a\ns:bcdefghij'|p:;\n\n:    x"; got != want {
		t.Errorf("wrapped:\n%s\nwant:\n%s", got, want)
	}
	// Runs count UTF-16 code units, as the browser does: an emoji is two.
	code = "x = '😀' + y"
	runs = []int32{4, 0, 4, class("s"), 3, 0, 1, class("nv")}
	if got, want := show(codeRows(code, runs, 80)), ":x = |s:'😀'|: + |nv:y"; got != want {
		t.Errorf("emoji: %s, want %s", got, want)
	}
	// Past the last run the code is plain; long code keeps its head and tail.
	code = strings.Repeat("x\n", 400)
	rows := codeRows(code, []int32{1, class("nv")}, 80)
	if len(rows) != 301 || rows[150] != nil || show(rows[:2]) != "nv:x\n:x" || show(rows[300:]) != ":x" {
		t.Errorf("clipped: %d rows: %s", len(rows), show(rows[:3]))
	}
}

// Highlighted, a Code call's code lays out as it would plain.
func TestHighlightedCodeKeepsItsLayout(t *testing.T) {
	m := &uiModel{styles: newStyles(false)}
	entry := codeEntry(1)
	entry.Tool.Input = "const files = ['cmd/kou-conveyor-tui/view.go', 'cmd/kou-conveyor-tui/view_test.go'];\n" +
		"for (const file of files) {\n\tconsole.log(file, (await readText(file)).length); // 😀 wide\n}"
	for _, width := range []int{40, 72, 120} {
		entry.Tool.Syntax = nil
		want := plain(m.renderTool(entry, width, true, false))
		entry.Tool.Syntax = highlight.Highlight("code.js", entry.Tool.Input, time.Time{}).Runs
		lines := m.renderTool(entry, width, true, false)
		if got := plain(lines); !slices.Equal(got, want) {
			t.Errorf("width %d:\n%s\nwant:\n%s", width, strings.Join(got, "\n"), strings.Join(want, "\n"))
		}
		for i, line := range lines {
			if w := ansi.StringWidth(line); w > width {
				t.Errorf("width %d: line %d is %d wide", width, i, w)
			}
		}
	}
}
