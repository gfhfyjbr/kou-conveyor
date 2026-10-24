package cockpit

import (
	"encoding/json/v2"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/highlight"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

func TestEditPlace(t *testing.T) {
	for _, c := range []struct {
		text               string
		line, replacements int
	}{
		{"Edited /w/a.go: replaced 1 occurrence at line 43. The file now reads around the first change:\n    39\tx", 43, 1},
		{"Edited /w/a.go: replaced 4 occurrences, the first at line 7.", 7, 4},
		// The path may say what the edit says; the edit says it last.
		{"Edited /w/replaced 2 occurrences at line 9.go: replaced 1 occurrence at line 5.", 5, 1},
		// What the file reads is not what the edit said.
		{"Edited /w/a.go.\n    3\treplaced 1 occurrence at line 3", 0, 0},
		{"", 0, 0},
	} {
		if line, replacements := editPlace(c.text); line != c.line || replacements != c.replacements {
			t.Errorf("editPlace(%q) = %d, %d, want %d, %d", c.text, line, replacements, c.line, c.replacements)
		}
	}
}

// classAt is the class of the code at offset, in UTF-16 units, of runs.
func classAt(runs []int32, offset int32) string {
	for i := 0; i+1 < len(runs); i += 2 {
		if offset < runs[i] {
			return highlight.Classes[runs[i+1]]
		}
		offset -= runs[i]
	}
	return ""
}

func TestEditDiff(t *testing.T) {
	for _, c := range []struct {
		name, old, replacement string
		line                   int
		want                   string
	}{
		{"a line changed", "func a() {\n\treturn 1\n}\n", "func a() {\n\treturn 2\n}\n", 12,
			"@@ -12,3 +12,3 @@\n func a() {\n-\treturn 1\n+\treturn 2\n }"},
		// Not knowing the line, the diff numbers the strings'.
		{"no line", "x := 1\n", "x := 2\n", 0, "@@ -1,1 +1,1 @@\n-x := 1\n+x := 2"},
		// Pieces of lines end where they do in both.
		{"pieces of lines", "x := 1", "x := 2", 5, "@@ -5,1 +5,1 @@\n-x := 1\n+x := 2"},
		{"lines added", "a()\nb()\n", "a()\nlog()\nb()\n", 3, "@@ -3,2 +3,3 @@\n a()\n+log()\n b()"},
		// Lines the two strings share stay whole, though they share pieces
		// with others.
		{"lines kept whole", "\tfoo(a)\n\tbar(b)\n\tbaz(c)\n", "\tbar(b)\n\tfoo(a)\n\tbaz(c)\n", 1, ""},
		// Far apart, changes are hunks of their own.
		{"two hunks", "a\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nb\n", "A\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nB\n", 10,
			"@@ -10,4 +10,4 @@\n-a\n+A\n 1\n 2\n 3\n@@ -18,4 +18,4 @@\n 8\n 9\n 10\n-b\n+B"},
		// Hunks a few lines apart are one.
		{"hunks joined", "a\n1\n2\n3\n4\n5\n6\n7\nb\n", "A\n1\n2\n3\n4\n5\n6\n7\nB\n", 10,
			"@@ -10,9 +10,9 @@\n-a\n+A\n 1\n 2\n 3\n 4\n 5\n 6\n 7\n-b\n+B"},
		// A hunk of two edits, the line between them kept, moves the lines
		// of the next by what it puts in and takes out: none here.
		{"a hunk of two edits", "a\nx\nb\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nc\n", "A\nx\nB\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nC\n", 1,
			"@@ -1,6 +1,6 @@\n-a\n+A\n x\n-b\n+B\n 1\n 2\n 3\n@@ -11,4 +11,4 @@\n 8\n 9\n 10\n-c\n+C"},
		{"lines put in, then taken out", "a\nx\nb\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nc\nd\n", "a\nn\nx\nn\nb\n1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nd\n", 1,
			"@@ -1,5 +1,7 @@\n a\n+n\n x\n+n\n b\n 1\n 2\n@@ -11,5 +13,4 @@\n 8\n 9\n 10\n-c\n d"},
		{"no change", "a\n", "a\n", 1, ""},
	} {
		diff, runs := editDiff("a.go", c.old, c.replacement, c.line, time.Time{})
		if c.name == "lines kept whole" {
			var removed, added, kept int
			for line := range strings.Lines(diff) {
				switch line[0] {
				case '-':
					removed++
				case '+':
					added++
				case ' ':
					kept++
				}
			}
			if removed != 1 || added != 1 || kept != 2 {
				t.Errorf("%s: a line moved is not a line taken out and put in elsewhere:\n%s", c.name, diff)
			}
		} else if diff != c.want {
			t.Errorf("%s: diff =\n%s\nwant\n%s", c.name, diff, c.want)
		}
		total := int32(0)
		for i := 0; i < len(runs); i += 2 {
			total += runs[i]
		}
		if total != highlight.Len(diff) {
			t.Errorf("%s: the runs cover %d units of the diff's %d", c.name, total, highlight.Len(diff))
		}
	}
	// The code shows in the colours of its tokens, signs and heads plain.
	diff, runs := editDiff("a.go", "func a() {\n\treturn 1\n}\n", "func a() {\n\treturn 2\n}\n", 12, time.Time{})
	at := func(s string) int32 { return highlight.Len(diff[:strings.Index(diff, s)]) }
	for _, c := range []struct {
		at   int32
		want string
	}{
		{0, ""},                        // the head
		{at("\n-") + 1, ""},            // a sign
		{at("func") - 1, ""},           // the sign of a line both share
		{at("func"), "kd"},             // a keyword of the string replaced
		{at("+\treturn") + 2, "k"},     // a keyword of the replacement
		{at("return 2") + 7, "m"},      // a number of the replacement
		{highlight.Len(diff) - 1, "p"}, // the last line's code
	} {
		if got := classAt(runs, c.at); got != c.want {
			t.Errorf("the class at %d of %q is %q, want %q", c.at, diff, got, c.want)
		}
	}
}

// edited is the diff an edit of a file that held content shows, the edit
// made by the harness, as a call makes it.
func edited(t *testing.T, content, old, replacement string, all bool) *FileView {
	t.Helper()
	path := filepath.Join(t.TempDir(), "a.go")
	if err := os.WriteFile(path, []byte(content), 0o600); err != nil {
		t.Fatal(err)
	}
	state := operation.FileState{Action: operation.FileEdit, Path: path, OldString: old, NewString: replacement, ReplaceAll: all}
	result, err := operation.RunFile(state)
	if err != nil {
		t.Fatal(err)
	}
	state.Result = &result
	return fileOpOf(state).view(highlightTime)
}

// An edit shows in the lines of its file: those it changed, whole, and
// those around them; where what it said does not tell how they read before
// it, in the lines of its strings.
func TestEditShowsInItsFile(t *testing.T) {
	lines := "l1\nl2\nl3\nl4\nl5\nl6\nl7\nl8\nl9\nl10\nl11\nl12\n"
	for _, c := range []struct {
		name, content, old, replacement string
		all                             bool
		want                            string
	}{
		{"whole lines", lines, "l6\n", "L6\nL6b\n", false, "@@ -3,7 +3,8 @@\n l3\n l4\n l5\n-l6\n+L6\n+L6b\n l7\n l8\n l9"},
		{"a piece of a line", "package a\n\nfunc f() int {\n\treturn g(1) + 2\n}\n", "g(1)", "g(10)", false,
			"@@ -1,5 +1,5 @@\n package a\n \n func f() int {\n-\treturn g(1) + 2\n+\treturn g(10) + 2\n }"},
		{"lines joined", "a\nb(\n\tx)\nc\n", "(\n\tx", "(x", false, "@@ -1,4 +1,3 @@\n a\n-b(\n-\tx)\n+b(x)\n c"},
		{"windows line endings", "a\r\nb\r\nc\r\n", "b\n", "B\n", false, "@@ -1,3 +1,3 @@\n a\n-b\n+B\n c"},
		// The snippet does not show the empty line after the last.
		{"the last line", "a\nb\n", "b\n", "B\nC\n", false, "@@ -1,2 +1,3 @@\n a\n-b\n+B\n+C"},
		{"a file without a newline at its end", "a\nb", "b", "B", false, "@@ -1,2 +1,2 @@\n a\n-b\n+B"},
		// The line the replacement is in has it twice: which one it is, the
		// edit does not say.
		{"two places it could be", "v := ab\n", "b", "a", false, "@@ -1,1 +1,1 @@\n-b\n+a"},
		// Nor where in a line a string taken out was.
		{"taken out of a line", "a\nb\nc\n", "b\n", "", false, "@@ -2,1 +2,0 @@\n-b"},
		{"taken out before an empty line", "a\nb\n\nc\n", "b\n", "", false, "@@ -1,4 +1,3 @@\n a\n-b\n \n c"},
		// Every occurrence replaced, the lines after the first may have been
		// changed too.
		{"replace all", "f(x)\nf(x)\n", "x", "y", true, "@@ -1,1 +1,1 @@\n-x\n+y"},
	} {
		view := edited(t, c.content, c.old, c.replacement, c.all)
		if view.Diff != c.want {
			t.Errorf("%s: diff =\n%s\nwant\n%s", c.name, view.Diff, c.want)
		}
	}
}

// Write and Edit calls show what they wrote and what they changed, the
// code in its colours; a failed edit what it tried.
func TestTranscriptShowsWhatFileCallsDo(t *testing.T) {
	marshal := func(v any) []byte {
		t.Helper()
		data, err := json.Marshal(v)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	sequence := 0
	item := func(kind sessionstore.ItemKind, data any, operations ...operation.Operation) []byte {
		sequence++
		return fileLine(t, "item", map[string]any{
			"Item":       sessionstore.Item{Sequence: sessionstore.Sequence(sequence), RecordedAt: recorded, Kind: kind, Data: data},
			"Operations": operations,
		})
	}
	file := func(id operation.ID, status operation.Status, state operation.FileState) operation.Operation {
		return operation.Operation{ID: id, Type: operation.TypeFile, Version: operation.VersionFile, Status: status, State: marshal(state)}
	}
	status := func(call string, o operation.Operation) []byte {
		return item(sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: "turn-1", CallID: call, Status: tool.CallStatus{WaitingFor: []operation.ID{o.ID}},
		}, o)
	}
	call := func(id, name, arguments string) llm.Item {
		return llm.Item{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: name, Arguments: arguments}}
	}
	edit := operation.FileState{
		Action: operation.FileEdit, Path: "/w/main.go", OldString: "func main() {\n\tprintln(1)\n}\n", NewString: "func main() {\n\tprintln(2)\n}\n",
		Result: &operation.FileResult{
			Text:  "Edited /w/main.go: replaced 1 occurrence at line 7. The file now reads around the first change:\n     7\tfunc main() {",
			Files: []string{"/w/main.go"}, Lines: 9,
		},
	}
	write := operation.FileState{
		Action: operation.FileWrite, Path: "/w/notes.py", Content: "x = 1\n",
		Result: &operation.FileResult{Text: "Wrote /w/notes.py: 1 line, 6 bytes.", Files: []string{"/w/notes.py"}, Lines: 1},
	}
	failed := operation.FileState{
		Action: operation.FileEdit, Path: "/w/main.go", OldString: "nope", NewString: "yes",
		TerminalError: "old_string was not found in /w/main.go; copy the lines exactly as Read shows them, without the line numbers",
	}
	read := operation.FileState{
		Action: operation.FileRead, Path: "/w/main.go", Result: &operation.FileResult{Text: "1\tpackage main", Lines: 1, From: 1, To: 1},
	}
	tr := NewTranscript()
	apply(t, tr,
		item(sessionstore.ItemTurn, session.Turn{ID: "turn-1", Type: session.TurnRegular}),
		item(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{Output: []llm.Item{
			call("call-1", "Edit", `{"path":"/w/main.go"}`),
			call("call-2", "Write", `{"path":"/w/notes.py"}`),
			call("call-3", "Edit", `{"path":"/w/main.go"}`),
			call("call-4", "Read", `{"path":"/w/main.go"}`),
		}}}),
		status("call-1", file("op-1", operation.StatusCompleted, edit)),
		status("call-2", file("op-2", operation.StatusCompleted, write)),
		status("call-3", file("op-3", operation.StatusFailed, failed)),
		status("call-4", file("op-4", operation.StatusCompleted, read)),
	)

	edited := tr.Entry("tool:call-1").Tool
	if c := edited.Change; c == nil || c.Kind != "edit" || c.Line != 7 || c.Content != "" || len(c.Syntax) == 0 ||
		c.Diff != "@@ -7,3 +7,3 @@\n func main() {\n-\tprintln(1)\n+\tprintln(2)\n }" {
		t.Fatalf("edit = %+v", c)
	}
	if !strings.HasPrefix(edited.Output, "Edited /w/main.go") {
		t.Fatalf("the edit's output = %q", edited.Output)
	}
	written := tr.Entry("tool:call-2").Tool
	if c := written.Change; c == nil || c.Kind != "write" || c.Content != "x = 1\n" || c.Diff != "" || len(c.Syntax) == 0 {
		t.Fatalf("write = %+v", c)
	}
	tried := tr.Entry("tool:call-3").Tool
	if c := tried.Change; tried.State != ToolFailed || !strings.Contains(tried.Error, "not found") ||
		c == nil || c.Line != 0 || c.Diff != "@@ -1,1 +1,1 @@\n-nope\n+yes" {
		t.Fatalf("failed edit = %+v, change %+v", tried, tried.Change)
	}
	if c := tr.Entry("tool:call-4").Tool.Change; c != nil {
		t.Fatalf("a read shows a change: %+v", c)
	}

	// What the call does is drawn once: the operation read again, the same,
	// draws nothing new.
	was := edited.Change
	apply(t, tr, fileLine(t, "operation", map[string]any{"Operation": file("op-1", operation.StatusCompleted, edit)}))
	if edited.Change != was {
		t.Fatal("the same edit was drawn again")
	}
	c := edited.clone()
	if c.Change == edited.Change || !reflect.DeepEqual(c.Change, edited.Change) {
		t.Fatal("a clone does not have a copy of the change")
	}

	got := string(marshal(written.Change))
	if !strings.HasPrefix(got, `{"kind":"write","content":"x = 1\n","syntax":[`) || strings.Contains(got, `"line"`) || strings.Contains(got, `"replacements"`) {
		t.Fatalf("write as JSON = %s", got)
	}
}
