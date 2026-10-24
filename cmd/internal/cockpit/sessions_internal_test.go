package cockpit

import (
	"bytes"
	"encoding/base64"
	"encoding/json/v2"
	"errors"
	"fmt"
	"image"
	"image/png"
	"io/fs"
	"os"
	"reflect"
	"slices"
	"sync"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/codevm"
	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// richSession returns the records of a session file that give a transcript
// something of all it keeps: a prompt with a picture and a file, tools that
// ran, failed, read a picture and a file, code with the calls it made, a
// tool still running, a compaction, a prompt no turn has taken yet and a
// record that cannot be read.
func richSession(t *testing.T) [][]byte {
	t.Helper()
	marshal := func(v any) []byte {
		t.Helper()
		data, err := json.Marshal(v)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	var picture bytes.Buffer
	if err := png.Encode(&picture, image.NewGray(image.Rect(0, 0, 3, 2))); err != nil {
		t.Fatal(err)
	}
	encoded := base64.StdEncoding.EncodeToString(picture.Bytes())
	prompt, err := contextbuilder.EncodeMessage(contextbuilder.Message{
		Text:   "look at $a.go and this",
		Images: []llm.Image{{Label: "shot.png", URL: "data:image/png;base64," + encoded}},
		Files:  []contextbuilder.LinkedFile{{Label: "$a.go", Path: "a.go", From: 1, To: 1, Lines: 1, Size: 9, Content: "package a"}},
	})
	if err != nil {
		t.Fatal(err)
	}
	sequence := 0
	item := func(kind sessionstore.ItemKind, data any, operations ...operation.Operation) []byte {
		sequence++
		return fileLine(t, "item", map[string]any{
			"Item": sessionstore.Item{
				Sequence: sessionstore.Sequence(sequence), RecordedAt: recorded.Add(time.Duration(sequence) * time.Second),
				Kind: kind, Data: data,
			},
			"Operations": operations,
		})
	}
	op := func(id operation.ID, kind operation.Type, version operation.Version, status operation.Status, state any) operation.Operation {
		return operation.Operation{ID: id, Type: kind, Version: version, Status: status, State: marshal(state)}
	}
	status := func(call string, operations ...operation.Operation) []byte {
		var waiting []operation.ID
		for _, o := range operations {
			waiting = append(waiting, o.ID)
		}
		return item(sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: "turn-1", CallID: call, Status: tool.CallStatus{WaitingFor: waiting},
		}, operations...)
	}
	checkpoint := func(o operation.Operation) []byte {
		return fileLine(t, "operation", map[string]any{"Operation": o})
	}
	shell := func(id operation.ID, command string, status operation.Status, result *operation.ShellResult) operation.Operation {
		return op(id, operation.TypeShell, operation.VersionShell, status, operation.ShellState{Input: operation.ShellInput{Command: command}, Result: result})
	}
	exit := func(code int) *int { return &code }
	started := recorded.Add(time.Minute)
	calls := []codevm.Call{
		{Index: 0, Name: "bash", Arguments: "go vet ./...", Output: "ok", ExitCode: exit(0), Started: started, Finished: started.Add(time.Second)},
		{Index: 1, Name: "readText", Arguments: "/work/go.mod", Started: started, Running: true},
	}
	code := func(status operation.Status, result *codevm.Result) operation.Operation {
		return op("op-2", operation.TypeCode, operation.VersionCode, status, operation.CodeState{
			Code: "await bash('go vet ./...')", Config: codevm.Config{Shell: "/bin/bash", Directory: "/work"}, Result: result,
		})
	}
	call := func(id, name, arguments string) llm.Item {
		return llm.Item{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: name, Arguments: arguments}}
	}
	answer := func(text string) llm.Response {
		return llm.Response{
			Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: text, Phase: "final_answer"}}},
			Usage:  llm.Usage{InputTokens: 900, OutputTokens: 20},
		}
	}
	return [][]byte{
		fileLine(t, "session", map[string]any{"Version": 2, "Session": session.Session{ID: "rich", CreatedAt: recorded}}),
		item(sessionstore.ItemInput, inbox.Input{ID: "in-1", Kind: inbox.InputExternal, Payload: prompt}),
		item(sessionstore.ItemTurn, session.Turn{ID: "turn-1", Type: session.TurnRegular, Model: "model-a"}),
		item(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-1", Response: llm.Response{
			Output: []llm.Item{
				{Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: []string{"**Looking**"}}},
				call("call-1", "Bash", `{"command":"go test ./..."}`),
				call("call-2", "Code", `{"code":"await bash('go vet ./...')"}`),
				call("call-3", "ViewImage", `{"path":"/work/shot.png"}`),
				call("call-4", "Read", `{"path":"/work/a.go"}`),
				call("call-5", "Bash", `{"command":"sleep 100"}`),
				call("call-6", "Edit", `{"path":"/work/a.go"}`),
			},
			Usage: llm.Usage{InputTokens: 100, CachedInputTokens: 60, OutputTokens: 7, ReasoningTokens: 3},
		}}),
		status("call-1", shell("op-1", "go test ./...", operation.StatusReady, nil)),
		checkpoint(shell("op-1", "go test ./...", operation.StatusCompleted, &operation.ShellResult{Out: "FAIL\n", Err: "exit status 1", ExitCode: 1})),
		status("call-2", code(operation.StatusAwaiting, &codevm.Result{Calls: calls, Started: started})),
		checkpoint(code(operation.StatusCompleted, &codevm.Result{
			Calls: calls, Error: "ReferenceError: x is not defined", Started: started, Finished: started.Add(2 * time.Second), Done: true,
		})),
		status("call-3", op("op-3", operation.TypeViewImage, operation.VersionViewImage, operation.StatusCompleted, operation.ViewImageState{
			Path: "/work/shot.png", Result: &operation.ViewImageResult{
				Content: encoded, OriginalWidth: 3, OriginalHeight: 2, OriginalMIMEType: "image/png", EncodedMIMEType: "image/png", ScaleRatio: 1,
			},
		})),
		status("call-4", op("op-4", operation.TypeFile, operation.VersionFile, operation.StatusCompleted, operation.FileState{
			Action: operation.FileRead, Path: "/work/a.go",
			Result: &operation.FileResult{Text: "1\tpackage a", Files: []string{"/work/a.go"}, Lines: 1, From: 1, To: 1},
		})),
		status("call-5", shell("op-5", "sleep 100", operation.StatusAwaiting, nil)),
		status("call-6", op("op-6", operation.TypeFile, operation.VersionFile, operation.StatusCompleted, operation.FileState{
			Action: operation.FileEdit, Path: "/work/a.go", OldString: "package a", NewString: "package b",
			Result: &operation.FileResult{
				Text:  "Edited /work/a.go: replaced 1 occurrence at line 1. The file now reads around the first change:\n     1\tpackage b",
				Files: []string{"/work/a.go"}, Lines: 1,
			},
		})),
		item(sessionstore.ItemTurn, session.Turn{ID: "turn-2", PreviousTurnID: "turn-1", Type: session.TurnRegular, Model: "model-a"}),
		item(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-2", Response: answer("Two of the five went wrong.")}),
		item(sessionstore.ItemTurn, session.Turn{ID: "turn-3", PreviousTurnID: "turn-2", Type: session.TurnCompaction}),
		item(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-3", Response: answer("<summary>We looked at a.go.</summary>")}),
		item(sessionstore.ItemInput, inbox.Input{ID: "in-2", Kind: inbox.InputExternal, Payload: marshal("and now?"), Delivery: inbox.DeliverAfterTools}),
		[]byte("{not json}"),
	}
}

// foldSession reads a session file afresh, as LoadSession did before it
// read on from where it stopped.
func foldSession(t *testing.T, path string) *Transcript {
	t.Helper()
	file, err := os.Open(path)
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	tr := NewTranscript()
	skipped := 0
	tr.Size, err = eachRecord(file, func(line []byte) {
		if _, err := tr.Apply(line); err != nil {
			skipped++
		}
	})
	if err != nil {
		t.Fatal(err)
	}
	tr.settle(skipped)
	return tr
}

// keptTranscript is the transcript LoadSession keeps of the session file at
// path, or nil.
func keptTranscript(path string) *Transcript {
	loaded.mu.Lock()
	defer loaded.mu.Unlock()
	if s := loaded.byPath[path]; s != nil {
		return s.t
	}
	return nil
}

// sameTranscript fails the test unless got is the transcript want is.
func sameTranscript(t *testing.T, got, want *Transcript, when string) {
	t.Helper()
	if reflect.DeepEqual(got, want) {
		return
	}
	entries := func(tr *Transcript) string {
		data, _ := json.Marshal(tr.Entries)
		return string(data)
	}
	t.Fatalf("%s: the transcript is not the one the file makes read afresh\n got: size %d, %+v, %q, %s\nwant: size %d, %+v, %q, %s",
		when, got.Size, got.Usage, got.Activity, entries(got), want.Size, want.Usage, want.Activity, entries(want))
}

// LoadSession reads on from where it stopped, as a run writes records, the
// half-written too, and comes to what reading the whole file afresh does.
func TestLoadSessionReadsOnWhatTheFileGained(t *testing.T) {
	for _, pictures := range []bool{true, false} {
		t.Run(fmt.Sprintf("KeepPictures=%v", pictures), func(t *testing.T) {
			was := KeepPictures
			KeepPictures = pictures
			t.Cleanup(func() { KeepPictures = was })
			dir := t.TempDir()
			path := SessionPath(dir, "rich")
			file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
			if err != nil {
				t.Fatal(err)
			}
			defer file.Close()
			var first *Transcript
			for i, line := range richSession(t) {
				half := len(line) / 2
				for j, part := range [][]byte{line[:half], append(slices.Clone(line[half:]), '\n')} {
					if _, err := file.Write(part); err != nil {
						t.Fatal(err)
					}
					when := fmt.Sprintf("record %d, part %d", i+1, j+1)
					got, err := LoadSession(dir, "rich")
					if err != nil {
						t.Fatalf("%s: %v", when, err)
					}
					sameTranscript(t, got, foldSession(t, path), when)
					if first == nil {
						first = keptTranscript(path)
					} else if keptTranscript(path) != first {
						t.Fatalf("%s: the file was read again from the start", when)
					}
				}
			}
		})
	}
}

// LoadSession, and a rewind, read a file whole as reading it a record at a
// time does, though they draw the tools its records change only once all
// are in: a Code call's code, what a Write wrote, and an Edit as it ended,
// not as it began.
func TestLoadSessionDrawsTheToolsOfAFileReadWhole(t *testing.T) {
	marshal := func(v any) []byte {
		t.Helper()
		data, err := json.Marshal(v)
		if err != nil {
			t.Fatal(err)
		}
		return data
	}
	sequence := 100
	item := func(kind sessionstore.ItemKind, data any, operations ...operation.Operation) []byte {
		sequence++
		return fileLine(t, "item", map[string]any{
			"Item": sessionstore.Item{
				Sequence: sessionstore.Sequence(sequence), RecordedAt: recorded.Add(time.Duration(sequence) * time.Second),
				Kind: kind, Data: data,
			},
			"Operations": operations,
		})
	}
	file := func(id operation.ID, status operation.Status, state operation.FileState) operation.Operation {
		return operation.Operation{ID: id, Type: operation.TypeFile, Version: operation.VersionFile, Status: status, State: marshal(state)}
	}
	status := func(call string, o operation.Operation) []byte {
		return item(sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: "turn-4", CallID: call, Status: tool.CallStatus{WaitingFor: []operation.ID{o.ID}},
		}, o)
	}
	checkpoint := func(o operation.Operation) []byte {
		return fileLine(t, "operation", map[string]any{"Operation": o})
	}
	call := func(id, name, arguments string) llm.Item {
		return llm.Item{Type: llm.ItemToolCall, Data: llm.ToolCall{CallID: id, Name: name, Arguments: arguments}}
	}
	write := operation.FileState{Action: operation.FileWrite, Path: "/work/b.go", Content: "package b\n\nfunc B() int { return 2 }\n"}
	edit := operation.FileState{Action: operation.FileEdit, Path: "/work/b.go", OldString: "return 2", NewString: "return 3"}
	edited := edit
	edited.Result = &operation.FileResult{
		Text:  "Edited /work/b.go: replaced 1 occurrence at line 3. The file now reads around the first change:\n     2\t\n     3\tfunc B() int { return 3 }",
		Files: []string{"/work/b.go"}, Lines: 2,
	}
	records := append(richSession(t),
		item(sessionstore.ItemTurn, session.Turn{ID: "turn-4", PreviousTurnID: "turn-3", Type: session.TurnRegular, Model: "model-a"}),
		item(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: "turn-4", Response: llm.Response{Output: []llm.Item{
			call("call-7", "Write", `{"path":"/work/b.go"}`),
			call("call-8", "Edit", `{"path":"/work/b.go"}`),
			call("call-9", "Code", `{"code":"const out = await bash('ls')"}`),
		}}}),
		status("call-7", file("op-7", operation.StatusReady, write)),
		checkpoint(file("op-7", operation.StatusCompleted, write)),
		status("call-8", file("op-8", operation.StatusReady, edit)),
		checkpoint(file("op-8", operation.StatusCompleted, edited)),
	)
	dir := t.TempDir()
	path := SessionPath(dir, "rich")
	data := append(bytes.Join(records, []byte{'\n'}), '\n')
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
	got, err := LoadSession(dir, "rich")
	if err != nil {
		t.Fatal(err)
	}
	want := foldSession(t, path)
	sameTranscript(t, got, want, "read whole")
	rewound, err := readTranscript(bytes.NewReader(data))
	if err != nil {
		t.Fatal(err)
	}
	sameTranscript(t, rewound, want, "read for a rewind")

	tools := map[string]*Tool{}
	for _, e := range got.Entries {
		if e.Tool != nil {
			tools[e.Tool.CallID] = e.Tool
		}
	}
	for _, id := range []string{"call-2", "call-9"} {
		if tools[id] == nil || len(tools[id].Syntax) == 0 {
			t.Errorf("%s: the code is not highlighted: %+v", id, tools[id])
		}
	}
	change := func(id string) *FileView {
		if tool := tools[id]; tool != nil {
			return tool.Change
		}
		return nil
	}
	if c := change("call-7"); c == nil || c.Kind != "write" || c.Content != write.Content || len(c.Syntax) == 0 {
		t.Errorf("the Write shows %+v", c)
	}
	if c := change("call-8"); c == nil || c.Kind != "edit" || c.Line != 3 || len(c.Syntax) == 0 {
		t.Errorf("the Edit shows %+v, not the edit as it ended", c)
	}
}

// A panic drawing a tool is the caller's, as it was when tools were drawn
// one by one, not the end of the program.
func TestDrawToolsPanicsInTheCaller(t *testing.T) {
	defer func() {
		if recover() == nil {
			t.Error("drawTools did not panic")
		}
	}()
	drawTools([]*Tool{{Name: "Code", Input: "1 + 1"}, nil, {Name: "Code", Input: "2"}})
}

// A file written over, cut short, replaced or gone is read again from the
// start, and so is one read as transcripts kept pictures they now do not.
func TestLoadSessionReadsAgainAFileThatIsNotTheOneRead(t *testing.T) {
	dir := t.TempDir()
	path := SessionPath(dir, "s1")
	load := func(when, title string) {
		t.Helper()
		got, err := LoadSession(dir, "s1")
		if err != nil {
			t.Fatalf("%s: %v", when, err)
		}
		sameTranscript(t, got, foldSession(t, path), when)
		if got.Title() != title {
			t.Fatalf("%s: title = %q, want %q", when, got.Title(), title)
		}
	}
	turn := string(fileLine(t, "item", map[string]any{"Item": sessionstore.Item{
		Sequence: 2, RecordedAt: recorded, Kind: sessionstore.ItemTurn, Data: session.Turn{ID: "turn-1", Type: session.TurnRegular},
	}})) + "\n"
	writeSession(t, dir, "s1", "hello", recorded, "")
	load("written", "hello")
	// As long as it was, with the same time: only its bytes tell.
	writeSession(t, dir, "s1", "howdy", recorded, "")
	load("written over", "howdy")
	writeSession(t, dir, "s1", "hey", recorded, turn)
	load("written over, longer", "hey")
	writeSession(t, dir, "s1", "yo", recorded, "")
	load("cut short", "yo")
	// Rewinding a session replaces its file.
	writeSession(t, dir, "s2", "rewound", recorded, turn)
	if err := os.Rename(SessionPath(dir, "s2"), path); err != nil {
		t.Fatal(err)
	}
	load("replaced", "rewound")
	if err := os.Remove(path); err != nil {
		t.Fatal(err)
	}
	if _, err := LoadSession(dir, "s1"); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("a session gone: %v", err)
	}
	if keptTranscript(path) != nil {
		t.Fatal("what was read of a file that is gone is kept")
	}
	writeSession(t, dir, "s1", "back", recorded, "")
	load("written again", "back")
	was := KeepPictures
	t.Cleanup(func() { KeepPictures = was })
	kept := keptTranscript(path)
	KeepPictures = !was
	load("pictures kept or not", "back")
	if keptTranscript(path) == kept {
		t.Fatal("a file read as transcripts kept pictures or not was read on as they do the other")
	}
	if err := DeleteSession(dir, "s1"); err != nil {
		t.Fatal(err)
	}
	if keptTranscript(path) != nil {
		t.Fatal("what was read of a deleted session is kept")
	}
}

// What LoadSession returns is the caller's: what a run does to it is not
// what the next caller gets.
func TestLoadSessionKeepsNothingTheCallerChanges(t *testing.T) {
	dir := t.TempDir()
	path := SessionPath(dir, "rich")
	var data []byte
	for _, line := range richSession(t) {
		data = append(append(data, line...), '\n')
	}
	if err := os.WriteFile(path, data, 0o600); err != nil {
		t.Fatal(err)
	}
	tr, err := LoadSession(dir, "rich")
	if err != nil {
		t.Fatal(err)
	}
	for _, e := range tr.Entries {
		e.Text, e.State, e.Model = "changed", Undelivered, "changed"
		for i := range e.Images {
			e.Images[i].Label = "changed"
		}
		for i := range e.Files {
			e.Files[i].Path = "changed"
		}
		tool := e.Tool
		if tool == nil {
			continue
		}
		tool.State, tool.Output = ToolCanceled, "changed"
		if tool.ExitCode != nil {
			*tool.ExitCode = 99
		}
		if tool.Image != nil {
			tool.Image.Label = "changed"
		}
		clear(tool.Syntax)
		if tool.Change != nil {
			tool.Change.Diff = "changed"
			clear(tool.Change.Syntax)
		}
		for i := range tool.Calls {
			tool.Calls[i].State = ToolCanceled
			if tool.Calls[i].ExitCode != nil {
				*tool.Calls[i].ExitCode = 99
			}
		}
		for i := range tool.Files {
			tool.Files[i] = "changed"
		}
		for i := range tool.outcomes {
			o := &tool.outcomes[i]
			o.status = operation.StatusFailed
			for j := range o.errors {
				o.errors[j] = "changed"
			}
			if o.exitCode != nil {
				*o.exitCode = 99
			}
			if o.info != nil {
				o.info.Label = "changed"
			}
			for j := range o.files {
				o.files[j] = "changed"
			}
			for j := range o.calls {
				if o.calls[j].ExitCode != nil {
					*o.calls[j].ExitCode = 99
				}
			}
		}
	}
	tr.Submit(messageID, "go on", recorded)
	apply(t, tr, liveLine(t, 30, sessionstore.ItemTurn, session.Turn{ID: "turn-9", Type: session.TurnRegular, Model: "model-b"}))
	tr.Live([]byte(`{"type":"delta","turn":"turn-9","index":0,"kind":"text","text":"Wor"}`))
	tr.Finish(errors.New("exit status 1"), true, recorded)
	clear(tr.Entries)

	again, err := LoadSession(dir, "rich")
	if err != nil {
		t.Fatal(err)
	}
	sameTranscript(t, again, foldSession(t, path), "loaded again")
}

// A copy of a transcript is the transcript, and shares nothing with it
// that changes: every field that reaches memory is copied.
func TestTranscriptCloneSharesNothing(t *testing.T) {
	tr := NewTranscript()
	for _, line := range richSession(t) {
		tr.Apply(line)
	}
	tr.Live([]byte(`{"type":"delta","turn":"turn-9","index":0,"kind":"text","text":"Wor"}`))
	c := tr.Clone()
	if !reflect.DeepEqual(c, tr) {
		t.Fatal("the copy is not the transcript")
	}
	original, copied := reach(tr), reach(c)
	for _, typ := range []reflect.Type{
		reflect.TypeFor[Transcript](), reflect.TypeFor[Entry](), reflect.TypeFor[Tool](),
		reflect.TypeFor[CodeCall](), reflect.TypeFor[outcome](), reflect.TypeFor[preview](),
	} {
		for i := range typ.NumField() {
			switch field := typ.Field(i); field.Type.Kind() {
			case reflect.Pointer, reflect.Map, reflect.Slice:
				if name := typ.Name() + "." + field.Name; !original.fields[name] {
					t.Errorf("the transcript has no %s: richSession should make one for Clone to copy", name)
				}
			}
		}
	}
	for address, field := range copied.at {
		if shared, ok := original.at[address]; ok {
			t.Errorf("the copy's %s is the transcript's %s", field, shared)
		}
	}
}

// reached is the memory a value reaches through pointers, maps and slices,
// by address, with the field that reaches it first, and the fields that
// reach any.
type reached struct {
	at     map[uintptr]string
	fields map[string]bool
}

func reach(v any) reached {
	r := reached{at: make(map[uintptr]string), fields: make(map[string]bool)}
	r.walk(reflect.ValueOf(v), "")
	return r
}

func (r reached) walk(v reflect.Value, field string) {
	switch v.Kind() {
	case reflect.Pointer, reflect.Map, reflect.Slice:
		// No time's location changes.
		if v.IsNil() || v.Kind() == reflect.Slice && v.Cap() == 0 || v.Type() == reflect.TypeFor[*time.Location]() {
			return
		}
		r.fields[field] = true
		if _, seen := r.at[v.Pointer()]; seen {
			return
		}
		r.at[v.Pointer()] = field
		switch v.Kind() {
		case reflect.Pointer:
			r.walk(v.Elem(), field)
		case reflect.Map:
			for entry := v.MapRange(); entry.Next(); {
				r.walk(entry.Key(), field)
				r.walk(entry.Value(), field)
			}
		default:
			for i := range v.Len() {
				r.walk(v.Index(i), field)
			}
		}
	case reflect.Struct:
		for i := range v.NumField() {
			r.walk(v.Field(i), v.Type().Name()+"."+v.Type().Field(i).Name)
		}
	case reflect.Interface:
		if !v.IsNil() {
			r.walk(v.Elem(), field)
		}
	}
}

// Front-ends load sessions from many goroutines at once, as runs write them,
// and more sessions than LoadSession keeps.
func TestLoadSessionConcurrently(t *testing.T) {
	dir := t.TempDir()
	ids := make([]string, loadedKept+2)
	for i := range ids {
		ids[i] = fmt.Sprintf("s%d", i)
		writeSession(t, dir, ids[i], "hello", recorded, "")
	}
	lines := richSession(t)
	var wg sync.WaitGroup
	wg.Go(func() {
		for _, line := range lines {
			for _, id := range ids {
				file, err := os.OpenFile(SessionPath(dir, id), os.O_WRONLY|os.O_APPEND, 0o600)
				if err != nil {
					t.Error(err)
					return
				}
				half := len(line) / 2
				file.Write(line[:half])
				file.Write(append(slices.Clone(line[half:]), '\n'))
				file.Close()
			}
		}
	})
	for range 8 {
		wg.Go(func() {
			for i := range 60 {
				if _, err := LoadSession(dir, ids[i%len(ids)]); err != nil {
					t.Error(err)
					return
				}
			}
		})
	}
	wg.Wait()
	for _, id := range ids {
		got, err := LoadSession(dir, id)
		if err != nil {
			t.Fatal(err)
		}
		sameTranscript(t, got, foldSession(t, SessionPath(dir, id)), id)
	}
	for i, id := range ids {
		if kept := keptTranscript(SessionPath(dir, id)) != nil; kept != (i >= len(ids)-loadedKept) {
			t.Errorf("session %s kept: %v", id, kept)
		}
	}
	loaded.mu.Lock()
	defer loaded.mu.Unlock()
	if len(loaded.byPath) != loadedKept || len(loaded.recent) != loadedKept {
		t.Fatalf("LoadSession keeps %d sessions, %d in order, not %d", len(loaded.byPath), len(loaded.recent), loadedKept)
	}
}
