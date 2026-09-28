package contextbuilder

import (
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

var linked = []LinkedFile{
	{Label: "$cmd/main.go", Path: "cmd/main.go", From: 1, To: 3, Lines: 345, Size: 10240, Content: "package main\n\nimport \"fmt\""},
	{Label: "$README.md", Path: "README.md", From: 1, To: 2, Lines: 2, Size: 30, Content: "# Title\nText."},
}

func TestMessagesCarryLinkedFiles(t *testing.T) {
	payload, err := EncodeMessage(Message{Text: "Look at $cmd/main.go and $README.md", Files: linked})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.HasPrefix(string(payload), `{"Text":"Look at $cmd/main.go and $README.md","Files":[{"Label":"$cmd/main.go","Path":"cmd/main.go","From":1,"To":3,"Lines":345,"Size":10240,`) {
		t.Fatalf("payload = %s", payload)
	}
	message, err := DecodeMessage(payload)
	if err != nil || message.Text != "Look at $cmd/main.go and $README.md" || !reflect.DeepEqual(message.Files, linked) || message.Images != nil {
		t.Fatalf("decoded %#v, %v", message, err)
	}
	// Readers of text and images read the text of a message with files.
	if text, images, err := ExternalMessage(payload); err != nil || text != message.Text || images != nil {
		t.Fatalf("ExternalMessage = %q, %v, %v", text, images, err)
	}
	// Text alone is still a JSON string.
	if payload, _ := EncodeMessage(Message{Text: "Hello"}); string(payload) != `"Hello"` {
		t.Fatalf("payload = %s", payload)
	}
	for _, bad := range []LinkedFile{{Label: "$x"}, {Path: "x", From: 2, To: 1}, {Path: "x", From: 1}, {Path: "x", To: 3}, {Path: "x", Lines: -1}} {
		if _, err := EncodeMessage(Message{Text: "x", Files: []LinkedFile{bad}}); err == nil {
			t.Errorf("file %#v was encoded", bad)
		}
	}
	for _, bad := range []string{`{"Text":"x","Files":[]}`, `{"Text":"x","Files":[{"Label":"$x"}]}`, `{"Text":"x","Files":[{"Path":"x","Other":1}]}`} {
		if _, err := DecodeMessage([]byte(bad)); err == nil {
			t.Errorf("payload %s decoded", bad)
		}
	}
}

func TestTheModelReadsLinkedFilesAfterTheText(t *testing.T) {
	payload, err := EncodeMessage(Message{Text: "Look at $cmd/main.go and $README.md", Files: linked})
	if err != nil {
		t.Fatal(err)
	}
	current := NewBuilder()
	if err := current.AddExternalInput(inbox.Input{ID: "input-1", Kind: inbox.InputExternal, Payload: payload}); err != nil {
		t.Fatal(err)
	}
	result, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	want := "Look at $cmd/main.go and $README.md\n\n<linked_files>\n" + linkedFilesPreface + `
<file path="cmd/main.go" label="$cmd/main.go">
Lines 1-3 of 345 (10.0 KB):
     1	package main
` + "     2\t" + `
     3	import "fmt"
Not shown: lines 4-345. Read them from the file when you need them, e.g. ` + "`sed -n '4,345p' cmd/main.go`" + `.
</file>
<file path="README.md" label="$README.md">
All 2 lines (30 bytes):
     1	# Title
     2	Text.
</file>
</linked_files>`
	got := messageText(result.Request.Input[1])
	if got != want {
		t.Fatalf("the model reads:\n%s\n\nwant:\n%s", got, want)
	}
	// What the files show counts toward the request's size.
	plain := NewBuilder()
	addInput(t, plain, "Look at $cmd/main.go and $README.md")
	without, _ := plain.Build()
	if added := result.EstimatedTokens - without.EstimatedTokens; added != estimateText(got)-estimateText("Look at $cmd/main.go and $README.md") {
		t.Fatalf("the files add %d tokens to the estimate", added)
	}
	if result.PendingTokens != result.EstimatedTokens-estimateItem(result.Request.Input[0]) {
		t.Fatalf("pending tokens = %d of %d", result.PendingTokens, result.EstimatedTokens)
	}
}

func TestLinkedFilesSayWhatTheyLeaveOut(t *testing.T) {
	for _, test := range []struct {
		name string
		file LinkedFile
		want string
	}{{
		name: "a range",
		file: LinkedFile{Label: "$a.go:10-11", Path: "a.go", From: 10, To: 11, Lines: 345, Size: 2048, Content: "x\ny"},
		want: `<file path="a.go" label="$a.go:10-11">
Lines 10-11 of 345 (2.0 KB):
    10	x
    11	y
Not shown: lines 1-9 and lines 12-345. Read them from the file when you need them, e.g. ` + "`sed -n '12,345p' a.go`" + `.
</file>`,
	}, {
		name: "the end of a file",
		file: LinkedFile{Path: "a.go", From: 1300, To: 1301, Lines: 1301, Size: 40000, Content: "x\ny"},
		want: `<file path="a.go">
Lines 1300-1301 of 1301 (39.1 KB):
  1300	x
  1301	y
Not shown: lines 1-1299. Read them from the file when you need them, e.g. ` + "`sed -n '300,1299p' a.go`" + `.
</file>`,
	}, {
		name: "a file too large to count",
		file: LinkedFile{Path: "big log.txt", From: 1, To: 1, Size: 3 << 30, Content: "first"},
		want: `<file path="big log.txt">
Line 1 of a file of 3.0 GB:
     1	first
Not shown: the lines after line 1. Read them from the file when you need them, e.g. ` + "`sed -n '2,1001p' 'big log.txt'`" + `.
</file>`,
	}, {
		name: "no lines",
		file: LinkedFile{Label: "$a.go:400-500", Path: "a.go", Lines: 345, Size: 2048},
		want: `<file path="a.go" label="$a.go:400-500">
No lines shown: the file has 345 lines (2.0 KB).
Read it from the file when you need it, e.g. ` + "`sed -n '1,345p' a.go`" + `.
</file>`,
	}, {
		name: "an empty file",
		file: LinkedFile{Path: "empty"},
		want: "<file path=\"empty\">\nThe file is empty.\n</file>",
	}, {
		name: "an image",
		file: LinkedFile{Path: "shot.PNG", Size: 5000, Binary: true},
		want: "<file path=\"shot.PNG\">\nA binary file of 4.9 KB, not shown. ViewImage shows an image.\n</file>",
	}, {
		name: "a file that could not be read",
		file: LinkedFile{Label: `$"it's <gone>"`, Path: "it's <gone>", Error: "no such file or directory"},
		want: "<file path=\"it's &lt;gone&gt;\" label=\"$&quot;it's &lt;gone&gt;&quot;\">\nNot read: no such file or directory.\n</file>",
	}, {
		name: "a folder",
		file: LinkedFile{Label: "$cmd/", Path: "cmd/", Directory: true, From: 1, To: 2, Lines: 5, Content: "internal/\nmain.go"},
		want: "<folder path=\"cmd/\" label=\"$cmd/\">\nEntries 1-2 of 5:\ninternal/\nmain.go\nNot shown: 3 more; list them with `ls -A cmd/`.\n</folder>",
	}, {
		name: "a whole folder",
		file: LinkedFile{Path: "a b/", Directory: true, From: 1, To: 1, Lines: 1, Content: "x"},
		want: "<folder path=\"a b/\">\n1 entry:\nx\n</folder>",
	}, {
		name: "an empty folder",
		file: LinkedFile{Path: "none/", Directory: true},
		want: "<folder path=\"none/\">\nThe folder is empty.\n</folder>",
	}} {
		t.Run(test.name, func(t *testing.T) {
			var b strings.Builder
			writeLinkedFile(&b, test.file)
			if b.String() != test.want {
				t.Fatalf("got:\n%s\n\nwant:\n%s", b.String(), test.want)
			}
		})
	}
}

func TestCompactionKeepsTheLabelsOfAnsweredLinkedFiles(t *testing.T) {
	current := NewBuilder()
	payload, err := EncodeMessage(Message{Text: "Fix $cmd/main.go", Files: linked[:1], Images: pasted[:1]})
	if err != nil {
		t.Fatal(err)
	}
	if err := current.AddExternalInput(inbox.Input{ID: "input-1", Kind: inbox.InputExternal, Payload: payload}); err != nil {
		t.Fatal(err)
	}
	current.Commit()
	current.AddModelResponse(answer("Fixed."))
	// A prompt the model has not answered keeps what its files show.
	payload, _ = EncodeMessage(Message{Text: "And $README.md?", Files: linked[1:]})
	if err := current.AddExternalInput(inbox.Input{ID: "input-2", Kind: inbox.InputExternal, Payload: payload}); err != nil {
		t.Fatal(err)
	}
	current.Commit()
	if !current.Compact(answer("<summary>Summary.</summary>")) {
		t.Fatal("did not compact")
	}
	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	summary := messageText(built.Request.Input[1])
	if !strings.Contains(summary, "<message>\nFix $cmd/main.go\n[The user attached [Image 1] here; images are left out after a compaction.]\n[The user linked $cmd/main.go here; what they showed is left out after a compaction: read the files themselves.]\n</message>") {
		t.Fatalf("kept prompts:\n%s", summary)
	}
	if strings.Contains(summary, "package main") {
		t.Fatalf("the summary keeps what an answered file showed:\n%s", summary)
	}
	last := built.Request.Input[len(built.Request.Input)-1].Data.(llm.Message)
	if last.Text != linkedText("And $README.md?", linked[1:]) {
		t.Fatalf("unanswered prompt = %#v", last)
	}
}
