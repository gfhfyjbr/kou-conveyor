package agentrunner

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"fmt"
	"os"
	"path/filepath"
	"reflect"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// numbered is n lines, "line 1" to "line n", each with its line break.
func numbered(n int) string {
	var b strings.Builder
	for i := 1; i <= n; i++ {
		fmt.Fprintf(&b, "line %d\n", i)
	}
	return b.String()
}

func writeFile(t *testing.T, dir, name, content string) {
	t.Helper()
	path := filepath.Join(dir, name)
	if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(path, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
}

func lines(from, to int) string {
	var parts []string
	for i := from; i <= to; i++ {
		parts = append(parts, fmt.Sprintf("line %d", i))
	}
	return strings.Join(parts, "\n")
}

func TestLinkedFilesShowWhatTheirLinksAskFor(t *testing.T) {
	workspace := t.TempDir()
	writeFile(t, workspace, "small.go", numbered(3))
	writeFile(t, workspace, "many.txt", numbered(wholeLines+1)) // few bytes, too many lines
	writeFile(t, workspace, "big.txt", numbered(5000))
	writeFile(t, workspace, "empty", "")
	writeFile(t, workspace, "crlf.txt", "one\r\ntwo\r\n")
	writeFile(t, workspace, "latin1.txt", "caf\xe9\n")
	writeFile(t, workspace, "noeol.txt", "a\nb")
	writeFile(t, workspace, "blob.bin", "\x89PNG\x00\x01\x02")
	size := func(name string) int64 {
		info, err := os.Stat(filepath.Join(workspace, name))
		if err != nil {
			t.Fatal(err)
		}
		return info.Size()
	}
	for _, test := range []struct {
		name    string
		request RequestFile
		want    contextbuilder.LinkedFile
	}{{
		name:    "a small file shows whole",
		request: RequestFile{Path: "small.go"},
		want:    contextbuilder.LinkedFile{Label: "$small.go", Path: "small.go", From: 1, To: 3, Lines: 3, Size: size("small.go"), Content: lines(1, 3)},
	}, {
		name:    "a file of many lines shows its beginning",
		request: RequestFile{Path: "./many.txt", Label: "$./many.txt"},
		want:    contextbuilder.LinkedFile{Label: "$./many.txt", Path: "many.txt", From: 1, To: previewLines, Lines: wholeLines + 1, Size: size("many.txt"), Content: lines(1, previewLines)},
	}, {
		name:    "a large file shows its beginning",
		request: RequestFile{Path: "big.txt"},
		want:    contextbuilder.LinkedFile{Label: "$big.txt", Path: "big.txt", From: 1, To: previewLines, Lines: 5000, Size: size("big.txt"), Content: lines(1, previewLines)},
	}, {
		name:    "from a line on",
		request: RequestFile{Path: "big.txt", StartLine: 4990},
		want:    contextbuilder.LinkedFile{Label: "$big.txt", Path: "big.txt", From: 4990, To: 5000, Lines: 5000, Size: size("big.txt"), Content: lines(4990, 5000)},
	}, {
		name:    "a range",
		request: RequestFile{Path: "big.txt", StartLine: 10, EndLine: 12, Label: "$big.txt:10-12"},
		want:    contextbuilder.LinkedFile{Label: "$big.txt:10-12", Path: "big.txt", From: 10, To: 12, Lines: 5000, Size: size("big.txt"), Content: lines(10, 12)},
	}, {
		name:    "a range too long to show whole",
		request: RequestFile{Path: "big.txt", StartLine: 1, EndLine: 4000},
		want:    contextbuilder.LinkedFile{Label: "$big.txt", Path: "big.txt", From: 1, To: rangeLines, Lines: 5000, Size: size("big.txt"), Content: lines(1, rangeLines)},
	}, {
		name:    "up to a line",
		request: RequestFile{Path: "big.txt", EndLine: 2},
		want:    contextbuilder.LinkedFile{Label: "$big.txt", Path: "big.txt", From: 1, To: 2, Lines: 5000, Size: size("big.txt"), Content: lines(1, 2)},
	}, {
		name:    "a range past the end",
		request: RequestFile{Path: "small.go", StartLine: 7, EndLine: 9},
		want:    contextbuilder.LinkedFile{Label: "$small.go", Path: "small.go", Lines: 3, Size: size("small.go")},
	}, {
		name:    "an empty file",
		request: RequestFile{Path: "empty"},
		want:    contextbuilder.LinkedFile{Label: "$empty", Path: "empty"},
	}, {
		name:    "Windows line breaks",
		request: RequestFile{Path: "crlf.txt"},
		want:    contextbuilder.LinkedFile{Label: "$crlf.txt", Path: "crlf.txt", From: 1, To: 2, Lines: 2, Size: size("crlf.txt"), Content: "one\ntwo"},
	}, {
		name:    "text that is not UTF-8",
		request: RequestFile{Path: "latin1.txt"},
		want:    contextbuilder.LinkedFile{Label: "$latin1.txt", Path: "latin1.txt", From: 1, To: 1, Lines: 1, Size: size("latin1.txt"), Content: "caf\uFFFD"},
	}, {
		name:    "no line break at the end",
		request: RequestFile{Path: "noeol.txt"},
		want:    contextbuilder.LinkedFile{Label: "$noeol.txt", Path: "noeol.txt", From: 1, To: 2, Lines: 2, Size: 3, Content: "a\nb"},
	}, {
		name:    "a binary file",
		request: RequestFile{Path: "blob.bin"},
		want:    contextbuilder.LinkedFile{Label: "$blob.bin", Path: "blob.bin", Size: size("blob.bin"), Binary: true},
	}, {
		name:    "a missing file",
		request: RequestFile{Path: "nope.go"},
		want:    contextbuilder.LinkedFile{Label: "$nope.go", Path: "nope.go", Error: "no such file or directory"},
	}} {
		t.Run(test.name, func(t *testing.T) {
			got := linkFile(workspace, test.request, messageBytes)
			if !reflect.DeepEqual(got, test.want) {
				t.Fatalf("linked\n%#v\nwant\n%#v", got, test.want)
			}
		})
	}
}

func TestLinkedFilesCutLongLines(t *testing.T) {
	workspace := t.TempDir()
	// A line of megabytes, cut in the middle of a rune.
	long := strings.Repeat("a", lineBytes-1) + "é" + strings.Repeat("b", 3<<20)
	writeFile(t, workspace, "min.js", long+"\nnext\n")
	got := linkFile(workspace, RequestFile{Path: "min.js"}, messageBytes)
	lines := strings.Split(got.Content, "\n")
	want := strings.Repeat("a", lineBytes-1) + fmt.Sprintf(" […%d more bytes]", len(long)-(lineBytes-1))
	if got.Lines != 2 || got.From != 1 || got.To != 2 || len(lines) != 2 || lines[0] != want || lines[1] != "next" {
		t.Fatalf("linked %d lines %d-%d:\n%.200q…", got.Lines, got.From, got.To, got.Content)
	}
}

func TestLinkedFoldersListTheirEntries(t *testing.T) {
	workspace := t.TempDir()
	writeFile(t, workspace, "cmd/b.go", "")
	writeFile(t, workspace, "cmd/A.go", "")
	writeFile(t, workspace, "cmd/zeta/x.go", "")
	writeFile(t, workspace, "cmd/.git/HEAD", "")
	writeFile(t, workspace, "cmd/.env", "")
	got := linkFile(workspace, RequestFile{Path: "cmd", Label: "$cmd"}, messageBytes)
	want := contextbuilder.LinkedFile{Label: "$cmd", Path: "cmd/", Directory: true, From: 1, To: 4, Lines: 4, Content: "zeta/\n.env\nA.go\nb.go"}
	if !reflect.DeepEqual(got, want) {
		t.Fatalf("linked\n%#v\nwant\n%#v", got, want)
	}
	for i := range folderEntries + 5 {
		writeFile(t, workspace, fmt.Sprintf("many/%03d", i), "")
	}
	got = linkFile(workspace, RequestFile{Path: "many/"}, messageBytes)
	if got.Path != "many/" || got.Lines != folderEntries+5 || got.From != 1 || got.To != folderEntries || strings.Count(got.Content, "\n") != folderEntries-1 {
		t.Fatalf("linked %d entries, %d-%d, path %q", got.Lines, got.From, got.To, got.Path)
	}
}

func TestLinkedFilesStayOutOfFIFOsAndDevices(t *testing.T) {
	workspace := t.TempDir()
	if err := syscall.Mkfifo(filepath.Join(workspace, "pipe"), 0o644); err != nil {
		t.Skip(err)
	}
	done := make(chan contextbuilder.LinkedFile, 1)
	go func() { done <- linkFile(workspace, RequestFile{Path: "pipe"}, messageBytes) }()
	select {
	case got := <-done:
		if got.Error != "not a regular file" {
			t.Fatalf("linked %#v", got)
		}
	case <-time.After(5 * time.Second):
		t.Fatal("reading a FIFO hangs")
	}
	if got := linkFile(workspace, RequestFile{Path: "/dev/zero"}, messageBytes); got.Error != "not a regular file" || got.Path != "/dev/zero" {
		t.Fatalf("linked %#v", got)
	}
}

func TestLinkedFilesOutsideTheWorkspace(t *testing.T) {
	home := t.TempDir()
	t.Setenv("HOME", home)
	writeFile(t, home, "notes.md", "note\n")
	got := linkFile(t.TempDir(), RequestFile{Path: "~/notes.md"}, messageBytes)
	// The model is told a path its commands can use as it is.
	if got.Path != filepath.Join(home, "notes.md") || got.Label != "$~/notes.md" || got.Content != "note" {
		t.Fatalf("linked %#v", got)
	}
	other := t.TempDir()
	writeFile(t, other, "x.txt", "x\n")
	got = linkFile(t.TempDir(), RequestFile{Path: filepath.Join(other, "x.txt")}, messageBytes)
	if got.Path != filepath.Join(other, "x.txt") || got.Content != "x" {
		t.Fatalf("linked %#v", got)
	}
}

func TestTheFilesOfAMessageShareABudget(t *testing.T) {
	workspace := t.TempDir()
	// Each of these shows rangeBytes at most: the third shows what is left
	// of the message's budget, and the fourth finds it spent.
	line := strings.Repeat("x", 99) + "\n"
	writeFile(t, workspace, "a.txt", strings.Repeat(line, 1000))
	var requests []RequestFile
	for range 4 {
		requests = append(requests, RequestFile{Path: "a.txt", StartLine: 1, EndLine: 1000})
	}
	linked := linkFiles(workspace, requests)
	shown := 0
	for _, file := range linked {
		shown += len(file.Content)
	}
	perRange := rangeBytes / len(line)
	left := (messageBytes - 2*(perRange*len(line)-1)) / len(line)
	if shown > messageBytes || linked[0].To != perRange || linked[1].To != perRange || linked[2].To != left || linked[3].From != 0 || linked[3].Lines != 1000 {
		t.Fatalf("shown %d bytes; lines %d-%d, %d-%d, %d-%d, %d-%d", shown,
			linked[0].From, linked[0].To, linked[1].From, linked[1].To, linked[2].From, linked[2].To, linked[3].From, linked[3].To)
	}
}

func TestValidateRequestChecksFiles(t *testing.T) {
	for _, test := range []struct {
		files []RequestFile
		want  string
	}{
		{[]RequestFile{{Path: " "}}, "file 1 has no path"},
		{[]RequestFile{{Path: "a\x00b"}}, "path must be one line without NUL bytes"},
		{[]RequestFile{{Path: "a", StartLine: -1}}, "lines count from 1"},
		{[]RequestFile{{Path: "a", StartLine: 5, EndLine: 2}}, "end_line 2 is before start_line 5"},
		{make([]RequestFile, maxLinkedFiles+1), "a message links at most 20 files"},
	} {
		_, err := validateRequest(Request{Messages: []RequestMessage{{Content: "hi", Files: test.files}}})
		if err == nil || !strings.Contains(err.Error(), test.want) || !strings.HasPrefix(err.Error(), "messages[0].files: ") {
			t.Errorf("error = %v, want %q", err, test.want)
		}
	}
}

func TestRunMainSendsTheFilesAMessageLinksToTheModel(t *testing.T) {
	workspace := t.TempDir()
	writeFile(t, workspace, "cmd/main.go", "package main\n\nfunc main() {}\n")
	requests := make(chan llm.Request, 1)
	client := &fakeClient{respond: func(_ context.Context, request llm.Request) (llm.Response, error) {
		requests <- request
		return llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{{
			Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "a main package"},
		}}}, nil
	}}
	var stdout, stderr bytes.Buffer
	code := RunMain(t.Context(), []string{"-workspace", workspace, "-session-directory", t.TempDir()},
		func(name string) string {
			return map[string]string{"OPENAI_API_KEY": "secret", "SHELL": "/bin/sh"}[name]
		},
		func() []string { return nil },
		strings.NewReader(`{"messages":[{"content":"What is $cmd/main.go:3 for?","files":[{"path":"cmd/main.go","start_line":3,"label":"$cmd/main.go:3"}]}]}`),
		&stdout, &stderr, testConfig(client))
	if code != 0 {
		t.Fatalf("exit = %d, stderr = %q, stdout = %q", code, stderr.String(), stdout.String())
	}
	request := <-requests
	text := request.Input[len(request.Input)-1].Data.(llm.Message).Text
	want := "What is $cmd/main.go:3 for?\n\n<linked_files>\n"
	if !strings.HasPrefix(text, want) || !strings.Contains(text, "<file path=\"cmd/main.go\" label=\"$cmd/main.go:3\">\nLine 3 of 3 (29 bytes):\n     3\tfunc main() {}\nNot shown: lines 1-2.") {
		t.Fatalf("the model reads:\n%s", text)
	}
	// The session records what the model read, for resuming.
	for _, item := range decodeLogItems(t, stdout.Bytes()) {
		if input, ok := item.Data.(inbox.Input); ok && input.Kind == inbox.InputExternal {
			message, err := contextbuilder.DecodeMessage(input.Payload)
			want := []contextbuilder.LinkedFile{{Label: "$cmd/main.go:3", Path: "cmd/main.go", From: 3, To: 3, Lines: 3, Size: 29, Content: "func main() {}"}}
			if err != nil || message.Text != "What is $cmd/main.go:3 for?" || !reflect.DeepEqual(message.Files, want) {
				t.Fatalf("recorded %#v, %v", message, err)
			}
			return
		}
	}
	t.Fatal("the prompt was not recorded")
}

func TestSteeringMessagesLinkFiles(t *testing.T) {
	workspace := t.TempDir()
	writeFile(t, workspace, "go.mod", "module x\n")
	input, err := steeringInput(jsontext.Value(`{"content":"see $go.mod","files":[{"path":"go.mod"}]}`), workspace)
	if err != nil {
		t.Fatal(err)
	}
	message, err := contextbuilder.DecodeMessage(input.Payload)
	if err != nil || len(message.Files) != 1 || message.Files[0].Content != "module x" || message.Files[0].Label != "$go.mod" {
		t.Fatalf("recorded %#v, %v", message, err)
	}
	if _, err := steeringInput(jsontext.Value(`{"content":"x","files":[{"path":""}]}`), workspace); err == nil {
		t.Fatal("a file without a path was taken")
	}
}
