// Package cockpit is the plumbing shared by the terminal and web front-ends
// of kou-conveyor-runner. It launches the runner, keeps two front-ends from
// running one session at the same time, and folds the runner's JSONL output
// and persisted session files into one display transcript.
package cockpit

import (
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"slices"
	"strings"
	"time"
	"unicode"

	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// Entry kinds.
const (
	KindUser      = "user"
	KindAssistant = "assistant"
	KindReasoning = "reasoning"
	KindTool      = "tool"
	KindNotice    = "notice"
	KindError     = "error"
)

// Tool states. Harness operation statuses are folded into these.
const (
	ToolQueued   = "queued"
	ToolRunning  = "running"
	ToolDone     = "done"
	ToolFailed   = "failed"
	ToolCanceled = "canceled"
)

// Delivery states of a prompt submitted by a front-end.
const (
	Pending     = "pending"     // handed to the runner, not persisted yet
	Undelivered = "undelivered" // the run ended before the runner persisted it
)

// Entry is one block of the transcript. IDs are derived from persisted
// identifiers, so the runner's live output and the session file produce the
// same IDs and front-ends can merge the two.
type Entry struct {
	ID     string    `json:"id"`
	Kind   string    `json:"kind"`
	At     time.Time `json:"at,omitzero"`
	Text   string    `json:"text,omitempty"`
	Detail string    `json:"detail,omitempty"` // notice body, shown on demand
	Phase  string    `json:"phase,omitempty"`  // assistant: commentary or final_answer
	State  string    `json:"state,omitempty"`  // user: Pending or Undelivered
	// Model is, for a prompt, the model that answered it: the one it was
	// sent to until the runner records the model its turns went to. A
	// session may use another model for every prompt.
	Model string `json:"model,omitempty"`
	// Forced marks a prompt sent while the agent worked, which it read
	// after the results of the tool calls it was making.
	Forced bool  `json:"forced,omitempty"`
	Tool   *Tool `json:"tool,omitempty"`

	// Images describes the images a prompt brought, which its text refers
	// to by their labels; their bytes stay in the session (PromptImages).
	Images []ImageInfo `json:"images,omitempty"`

	// Rev increases whenever the entry changes, so renderers can cache.
	Rev int `json:"-"`
}

// Tool is the call and the outcome of one tool invocation.
type Tool struct {
	CallID   string    `json:"call_id"`
	Name     string    `json:"name,omitempty"`
	Input    string    `json:"input,omitempty"` // the gist of the arguments: command, path or skill
	State    string    `json:"state"`
	ExitCode *int      `json:"exit_code,omitempty"`
	Output   string    `json:"output,omitempty"`
	Stderr   string    `json:"stderr,omitempty"`
	Error    string    `json:"error,omitempty"`
	Started  time.Time `json:"started,omitzero"`
	Finished time.Time `json:"finished,omitzero"`
	// Image describes the picture a ViewImage call read, as the model got
	// it; its bytes stay with the call (Picture, ToolImage).
	Image *ImageInfo `json:"image,omitempty"`

	callError  string
	operations []operation.Operation
}

// Terminal reports whether the tool will not change state any more.
func (tool *Tool) Terminal() bool {
	return tool.State == ToolDone || tool.State == ToolFailed || tool.State == ToolCanceled
}

// Usage accumulates model token usage.
type Usage struct {
	Input     int64 `json:"input"`
	Cached    int64 `json:"cached"`
	Output    int64 `json:"output"`
	Reasoning int64 `json:"reasoning"`
	Context   int64 `json:"context"` // input tokens of the latest model request
	Turns     int   `json:"turns"`
}

// Transcript folds runner events into entries. It is not safe for concurrent
// use; each front-end owns its transcripts from a single goroutine.
type Transcript struct {
	Entries []*Entry
	Usage   Usage
	// Activity describes what the runner is doing, e.g. "Thinking".
	Activity string
	// Size is how many bytes of the session file LoadSession read; the file
	// only grows, so a different size means there is more to read.
	Size int64

	index      map[string]*Entry
	operations map[operation.ID]string // operation → tool call
	compaction map[session.TurnID]bool
	active     map[string]*Entry // tools that have not finished
	// awaiting are the prompts no turn has answered yet: the next turn's
	// model is theirs.
	awaiting []*Entry
	failures int    // error entries produced by runner output
	runMark  int    // failures when the current run started
	run      string // message ID of the current run
	serial   int
	// compactable is set by the model's answers and cleared by a compaction.
	compactable bool
}

func NewTranscript() *Transcript {
	return &Transcript{
		index:      make(map[string]*Entry),
		operations: make(map[operation.ID]string),
		compaction: make(map[session.TurnID]bool),
		active:     make(map[string]*Entry),
	}
}

// Entry returns the entry with the given ID, or nil.
func (t *Transcript) Entry(id string) *Entry { return t.index[id] }

// Title is the first prompt of the transcript, on one line.
func (t *Transcript) Title() string {
	for _, e := range t.Entries {
		if e.Kind == KindUser {
			return Headline(e.Text, 80)
		}
	}
	return ""
}

// Running returns the tools that have not finished, oldest first.
func (t *Transcript) Running() []*Entry {
	var running []*Entry
	for _, e := range t.Entries {
		if t.active[e.ID] != nil {
			running = append(running, e)
		}
	}
	return running
}

// Submit records a prompt a front-end is about to hand to the runner and
// marks the start of a run. The entry settles when the runner persists the
// input with the same message ID.
func (t *Transcript) Submit(messageID, text string, at time.Time) *Entry {
	t.runMark, t.run = t.failures, messageID
	t.Activity = "Starting"
	e := &Entry{ID: "input:" + messageID, Kind: KindUser, At: at, Text: Clean(text), State: Pending}
	t.put(e)
	t.awaiting = append(t.awaiting, e)
	return e
}

// SubmitModel is Submit for a prompt sent to model, which its entry shows
// until the runner says which model answered it.
func (t *Transcript) SubmitModel(messageID, text, model string, at time.Time) *Entry {
	e := t.Submit(messageID, text, at)
	e.Model = strings.TrimSpace(model)
	return e
}

// LastModel is the model that answered the latest prompt that says, or "".
func (t *Transcript) LastModel() string {
	for i := len(t.Entries) - 1; i >= 0; i-- {
		if e := t.Entries[i]; e.Kind == KindUser && e.Model != "" && e.State != Undelivered {
			return e.Model
		}
	}
	return ""
}

// Begin marks the start of a run that sends no prompt, such as a compaction,
// under an ID of the front-end's choosing, and says what it is doing.
func (t *Transcript) Begin(runID, activity string) {
	t.runMark, t.run = t.failures, runID
	t.Activity = activity
}

// Compactable reports that compacting the session would summarize
// something: the model answered since the conversation was last compacted.
func (t *Transcript) Compactable() bool { return t.compactable }

// Finish settles the transcript when a run ends. Prompts the runner never
// persisted become undelivered, a stopped run is marked, and an abnormal exit
// the runner did not report itself becomes an error entry.
func (t *Transcript) Finish(err error, stopped bool, at time.Time) []*Entry {
	var changed []*Entry
	for _, e := range t.Entries {
		if e.Kind == KindUser && e.State == Pending {
			e.State = Undelivered
			changed = append(changed, t.touch(e))
		}
	}
	for _, e := range t.Running() {
		e.Tool.State = ToolCanceled
		delete(t.active, e.ID)
		changed = append(changed, t.touch(e))
	}
	switch {
	case stopped:
		changed = append(changed, t.put(&Entry{
			ID: t.liveID("stopped"), Kind: KindNotice, At: at, Text: "Run stopped",
		}))
	case err != nil && t.failures == t.runMark:
		changed = append(changed, t.put(&Entry{
			ID: t.liveID("exit"), Kind: KindError, At: at, Text: "Runner exited: " + Clean(err.Error()),
		}))
	}
	// A prompt the runner never took is answered by no turn.
	t.awaiting = slices.DeleteFunc(t.awaiting, func(e *Entry) bool { return e.State == Undelivered })
	t.Activity = ""
	return changed
}

// Apply folds one line of runner output or of a session file into the
// transcript and returns the entries it created or changed.
func (t *Transcript) Apply(line []byte) ([]*Entry, error) {
	line = bytes.TrimSpace(line)
	if len(line) == 0 {
		return nil, nil
	}
	var envelope struct {
		Type    string         `json:"type"`
		Message string         `json:"message"`
		Data    jsontext.Value `json:"data"`
	}
	if err := json.Unmarshal(line, &envelope); err != nil {
		return nil, fmt.Errorf("decode runner event: %w", err)
	}
	switch envelope.Type {
	case "":
		// Live runner output is one session item per line.
		var item sessionstore.Item
		if err := json.Unmarshal(line, &item); err != nil {
			return nil, fmt.Errorf("decode session item: %w", err)
		}
		return t.applyItem(item), nil
	case "error":
		t.failures++
		return []*Entry{t.put(&Entry{
			ID: t.liveID("error"), Kind: KindError, At: time.Now().UTC(), Text: Clean(envelope.Message),
		})}, nil
	case "session":
		return nil, nil
	case "item":
		var record struct {
			Item       sessionstore.Item
			Operations []operation.Operation
		}
		if err := json.Unmarshal(envelope.Data, &record); err != nil {
			return nil, fmt.Errorf("decode session record: %w", err)
		}
		// Session files keep a status's operations next to the item.
		if status, ok := record.Item.Data.(sessionstore.ToolCallStatus); ok && len(status.Operations) == 0 {
			status.Operations = record.Operations
			record.Item.Data = status
		}
		return t.applyItem(record.Item), nil
	case "operation":
		var record struct{ Operation operation.Operation }
		if err := json.Unmarshal(envelope.Data, &record); err != nil {
			return nil, fmt.Errorf("decode operation record: %w", err)
		}
		return t.applyOperation(record.Operation), nil
	default:
		return nil, fmt.Errorf("unsupported record type %q", envelope.Type)
	}
}

func (t *Transcript) applyItem(item sessionstore.Item) []*Entry {
	at := item.RecordedAt
	switch data := item.Data.(type) {
	case sessionstore.Fork:
		return []*Entry{t.put(&Entry{
			ID: fmt.Sprintf("fork:%d", item.Sequence), Kind: KindNotice, At: at,
			Text: "Forked from session " + Clean(string(data.ParentID)),
		})}
	case inbox.Input:
		if data.Kind != inbox.InputExternal {
			return nil
		}
		id := "input:" + string(data.ID)
		text, images := payloadEntry(data.Payload)
		forced := data.Delivery == inbox.DeliverAfterTools
		if e := t.index[id]; e != nil {
			e.At, e.State, e.Forced = at, "", forced
			if text != "" {
				e.Text = text
			}
			if images != nil {
				e.Images = images
			}
			t.await(e)
			return []*Entry{t.touch(e)}
		}
		e := t.put(&Entry{ID: id, Kind: KindUser, At: at, Text: text, Forced: forced, Images: images})
		t.await(e)
		return []*Entry{e}
	case session.Turn:
		if data.Type == session.TurnCompaction {
			t.compaction[data.ID] = true
			t.Activity = "Compacting context"
		} else {
			t.Activity = "Thinking"
		}
		// The prompts waiting for this turn were answered by its model.
		var changed []*Entry
		for _, e := range t.awaiting {
			if data.Model != "" && e.Model != data.Model {
				e.Model = data.Model
				changed = append(changed, t.touch(e))
			}
		}
		t.awaiting = t.awaiting[:0]
		return changed
	case sessionstore.ModelResponse:
		return t.applyResponse(uint64(item.Sequence), at, data)
	case sessionstore.ToolCallStatus:
		e := t.tool(data.CallID, at)
		e.Tool.callError = data.Status.Error
		for _, value := range data.Operations {
			t.operations[value.ID] = data.CallID
			e.Tool.operations = mergeOperation(e.Tool.operations, value)
		}
		t.refresh(e, at)
		return []*Entry{t.touch(e)}
	}
	return nil
}

func (t *Transcript) applyResponse(sequence uint64, at time.Time, data sessionstore.ModelResponse) []*Entry {
	response := data.Response
	t.Usage.Input += response.Usage.InputTokens
	t.Usage.Cached += response.Usage.CachedInputTokens
	t.Usage.Output += response.Usage.OutputTokens
	t.Usage.Reasoning += response.Usage.ReasoningTokens
	t.Usage.Context = response.Usage.InputTokens
	t.Usage.Turns++

	var changed []*Entry
	if t.compaction[data.TurnID] {
		// The summary is what the runner keeps of the answer: the model
		// thinks the conversation through before it.
		summary := contextbuilder.Summary(response)
		notice := &Entry{ID: fmt.Sprintf("compaction:%d", sequence), Kind: KindNotice, At: at}
		if summary == "" {
			// The runner keeps the conversation as it was and goes on.
			notice.Text = "Context not compacted: the model wrote no summary"
		} else {
			notice.Text, notice.Detail = "Context compacted", Clean(summary)
			if before := response.Usage.InputTokens; before > 0 {
				notice.Text += " from " + approximateTokens(before) + " tokens"
			}
			// The conversation is now about the size of the answer, which
			// the prompts kept with the summary make up for, until the next
			// turn reports it.
			t.Usage.Context = max(response.Usage.OutputTokens-response.Usage.ReasoningTokens, int64(len(notice.Detail)/4))
			t.compactable = false
		}
		changed = append(changed, t.put(notice))
	} else {
		t.compactable = t.compactable || len(response.Output) != 0
		calls := 0
		for index, output := range response.Output {
			id := fmt.Sprintf("output:%d.%d", sequence, index)
			switch value := output.Data.(type) {
			case llm.Message:
				text := Clean(value.Text)
				if value.Role != llm.RoleAssistant && value.Role != "" || strings.TrimSpace(text) == "" {
					continue
				}
				changed = append(changed, t.put(&Entry{ID: id, Kind: KindAssistant, At: at, Text: text, Phase: value.Phase}))
			case llm.Reasoning:
				text := Clean(strings.Join(value.Summary, "\n\n"))
				if strings.TrimSpace(text) == "" {
					continue
				}
				changed = append(changed, t.put(&Entry{ID: id, Kind: KindReasoning, At: at, Text: text}))
			case llm.ToolCall:
				calls++
				e := t.tool(value.CallID, at)
				e.Tool.Name = Clean(value.Name)
				if input := toolInput(value.Arguments); input != "" {
					e.Tool.Input = input
				}
				changed = append(changed, t.touch(e))
			}
		}
		if calls == 0 {
			t.Activity = "Wrapping up"
		} else {
			t.Activity = t.runningActivity()
		}
	}
	if failure := response.Failure; failure != nil {
		text := failure.Message
		if failure.Code != "" {
			text = failure.Code + ": " + text
		}
		t.failures++
		changed = append(changed, t.put(&Entry{
			ID: fmt.Sprintf("failure:%d", sequence), Kind: KindError, At: at, Text: Clean(text),
		}))
	}
	switch response.Stop {
	case llm.StopMaxOutputTokens:
		changed = append(changed, t.put(&Entry{
			ID: fmt.Sprintf("stop:%d", sequence), Kind: KindNotice, At: at, Text: "Response cut off at the output token limit",
		}))
	case llm.StopRefused:
		changed = append(changed, t.put(&Entry{
			ID: fmt.Sprintf("stop:%d", sequence), Kind: KindNotice, At: at, Text: "The model refused to respond",
		}))
	}
	return changed
}

// applyOperation handles the operation checkpoints of a session file, which
// carry tool progress between the two statuses recorded for each call.
func (t *Transcript) applyOperation(value operation.Operation) []*Entry {
	callID, ok := t.operations[value.ID]
	if !ok {
		return nil
	}
	e := t.index["tool:"+callID]
	if e == nil {
		return nil
	}
	e.Tool.operations = mergeOperation(e.Tool.operations, value)
	t.refresh(e, time.Time{})
	return []*Entry{t.touch(e)}
}

// tool returns the entry of a tool call, creating it when a status arrives
// before (or without) the call itself.
func (t *Transcript) tool(callID string, at time.Time) *Entry {
	id := "tool:" + callID
	if e := t.index[id]; e != nil {
		return e
	}
	e := &Entry{ID: id, Kind: KindTool, At: at, Tool: &Tool{CallID: callID, State: ToolQueued, Started: at}}
	t.put(e)
	t.active[id] = e
	return e
}

func (t *Transcript) refresh(e *Entry, at time.Time) {
	tool := e.Tool
	tool.Output, tool.Stderr, tool.Error, tool.ExitCode, tool.Image = "", "", "", nil, nil
	state := ToolDone
	if tool.callError != "" {
		state, tool.Error = ToolFailed, Clean(tool.callError)
	}
	var outputs, errors []string
	for _, value := range tool.operations {
		switch value.Status {
		case operation.StatusCompleted:
		case operation.StatusFailed:
			state = ToolFailed
		case operation.StatusCanceled, operation.StatusCanceling:
			if state != ToolFailed {
				state = ToolCanceled
			}
		default:
			if state == ToolDone {
				state = ToolRunning
			}
		}
		output, stderr := describeOperation(tool, value)
		if output != "" {
			outputs = append(outputs, output)
		}
		if stderr != "" {
			errors = append(errors, stderr)
		}
	}
	tool.Output = Clean(strings.Join(outputs, "\n"))
	tool.Stderr = Clean(strings.Join(errors, "\n"))
	tool.State = state
	if tool.Terminal() {
		delete(t.active, e.ID)
		if tool.Finished.IsZero() && !at.IsZero() {
			tool.Finished = at
		}
	} else {
		t.active[e.ID] = e
	}
	t.Activity = t.runningActivity()
}

func (t *Transcript) runningActivity() string {
	if len(t.active) != 1 {
		if len(t.active) == 0 {
			return "Thinking"
		}
		return fmt.Sprintf("Running %d tools", len(t.active))
	}
	for _, e := range t.active {
		if e.Tool.Name != "" {
			return "Running " + e.Tool.Name
		}
	}
	return "Running a tool"
}

func describeOperation(tool *Tool, value operation.Operation) (output, stderr string) {
	if len(value.State) == 0 {
		return "", ""
	}
	switch value.Type {
	case operation.TypeShell:
		var state operation.ShellState
		if json.Unmarshal(value.State, &state) != nil {
			return "", ""
		}
		if tool.Input == "" {
			tool.Input = Clean(state.Input.Command)
		}
		if state.TerminalError != "" {
			tool.Error = joinLines(tool.Error, Clean(state.TerminalError))
		}
		if state.Result != nil {
			code := state.Result.ExitCode
			tool.ExitCode = &code
			return state.Result.Out, state.Result.Err
		}
	case operation.TypeViewImage:
		var state operation.ViewImageState
		if json.Unmarshal(value.State, &state) != nil {
			return "", ""
		}
		if tool.Input == "" {
			tool.Input = Clean(state.Path)
		}
		if result := state.Result; result != nil {
			if result.Error != "" {
				tool.Error = joinLines(tool.Error, Clean(result.Error))
				return "", ""
			}
			if value.Status == operation.StatusCompleted {
				tool.Image = viewedImage(state)
			}
			return fmt.Sprintf("%d×%d %s", result.OriginalWidth, result.OriginalHeight, result.OriginalMIMEType), ""
		}
	case operation.TypeSkillUse:
		var state operation.SkillUseState
		if json.Unmarshal(value.State, &state) != nil {
			return "", ""
		}
		if tool.Input == "" {
			tool.Input = Clean(state.Path)
		}
		if state.TerminalError != "" {
			tool.Error = joinLines(tool.Error, Clean(state.TerminalError))
		} else if value.Status == operation.StatusCompleted {
			return "Loaded " + state.Path, ""
		}
	}
	return "", ""
}

func mergeOperation(operations []operation.Operation, value operation.Operation) []operation.Operation {
	for i := range operations {
		if operations[i].ID == value.ID {
			operations[i] = value
			return operations
		}
	}
	return append(operations, value)
}

// await notes a prompt that the next turn answers.
func (t *Transcript) await(e *Entry) {
	if !slices.Contains(t.awaiting, e) {
		t.awaiting = append(t.awaiting, e)
	}
}

// liveID names an entry that exists only in live output. It includes the run,
// so entries of different runs never share an ID, even when every run is
// folded into a fresh transcript as the web server does.
func (t *Transcript) liveID(kind string) string {
	t.serial++
	if t.run == "" {
		return fmt.Sprintf("%s:%d", kind, t.serial)
	}
	return fmt.Sprintf("%s:%s.%d", kind, t.run, t.serial)
}

// put appends a new entry, or replaces the content of an existing entry with
// the same ID in place.
func (t *Transcript) put(e *Entry) *Entry {
	if existing := t.index[e.ID]; existing != nil {
		rev := existing.Rev
		*existing = *e
		existing.Rev = rev + 1
		return existing
	}
	t.Entries = append(t.Entries, e)
	t.index[e.ID] = e
	return e
}

func (t *Transcript) touch(e *Entry) *Entry {
	e.Rev++
	return e
}

// toolInput extracts what a person wants to see from tool arguments.
func toolInput(arguments string) string {
	var fields map[string]jsontext.Value
	if json.Unmarshal([]byte(arguments), &fields) != nil {
		return Headline(Clean(arguments), 400)
	}
	for _, name := range []string{"command", "path", "name"} {
		var value string
		if raw, ok := fields[name]; ok && json.Unmarshal(raw, &value) == nil && value != "" {
			return Clean(value)
		}
	}
	return Headline(Clean(arguments), 400)
}

// payloadText is the text of a prompt's payload: text alone, or the text
// of a prompt that brought images.
func payloadText(payload jsontext.Value) string {
	text, _ := payloadMessage(payload)
	return Clean(text)
}

// Clean makes untrusted text safe to display: it drops terminal escapes and
// control characters, repairs invalid UTF-8 and resolves carriage returns the
// way a terminal would.
func Clean(s string) string {
	s = ansi.Strip(strings.ToValidUTF8(s, "\uFFFD"))
	s = strings.ReplaceAll(s, "\r\n", "\n")
	if strings.ContainsRune(s, '\r') {
		lines := strings.Split(s, "\n")
		for i, line := range lines {
			if cut := strings.LastIndexByte(strings.TrimRight(line, "\r"), '\r'); cut >= 0 {
				line = line[cut+1:]
			}
			lines[i] = line
		}
		s = strings.Join(lines, "\n")
	}
	return strings.Map(func(r rune) rune {
		if r == '\n' || r == '\t' {
			return r
		}
		if unicode.IsControl(r) {
			return -1
		}
		return r
	}, s)
}

// Headline collapses whitespace and shortens s to at most limit runes.
func Headline(s string, limit int) string {
	s = strings.Join(strings.Fields(s), " ")
	if r := []rune(s); len(r) > limit {
		return string(r[:max(0, limit-1)]) + "…"
	}
	return s
}

// approximateTokens writes a token count the way people say it: 812k, 1.2M.
func approximateTokens(n int64) string {
	switch {
	case n < 1000:
		return fmt.Sprint(n)
	case n < 999_500:
		return fmt.Sprintf("%dk", (n+500)/1000)
	}
	return strings.TrimSuffix(fmt.Sprintf("%.1f", float64(n)/1e6), ".0") + "M"
}

func joinLines(a, b string) string {
	if a == "" {
		return b
	}
	return a + "\n" + b
}
