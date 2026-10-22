package cockpit

import (
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// Editing a prompt that was already sent rewinds its session: the session
// file goes back to how it was before the prompt's run began, and the edited
// prompt runs from there under the same session ID. What remains is a state
// the session really was in, so the runner resumes it like any other.

// ErrPromptGone reports a prompt a session does not hold: the runner never
// persisted it, or the session was rewound past it elsewhere.
var ErrPromptGone = errors.New("the prompt is no longer in the session")

// RewindSession takes a session back to how it was before the prompt with the
// given message ID was sent. The prompt's run and everything after it leave
// the session, with the output their commands left behind. Commands started
// before the prompt keep the outcome they reached later, so resuming never
// runs one twice. The model's notes and the summaries of the session's
// compactions go back too (restoreMemory). Files the agent changed in the
// workspace stay as they are.
func RewindSession(dir, id, messageID string) error {
	unlock, err := LockSession(dir, id)
	if err != nil {
		return err
	}
	defer unlock()
	return rewindSession(dir, id, messageID, "")
}

// rewindSession is RewindSession for a caller that holds the session's lock.
// next is the message ID of the prompt the caller runs in the rewound
// session, "" for none.
func rewindSession(dir, id, messageID, next string) error {
	if !ValidSessionID(id) {
		return fmt.Errorf("invalid session ID %q", id)
	}
	path := SessionPath(dir, id)
	data, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	cut, err := cutLog(data, messageID, time.Now().UTC())
	if err != nil {
		return err
	}
	// The model's notes and the summaries of the session's compactions are
	// kept beside the log, and the harness shows the model the notes and
	// points it at both: they go back to how they were before the prompt's
	// run, so that the rewound run does not read what its discarded future
	// wrote (memory.go). This comes before the log is cut, so that a failure
	// leaves the session as it was.
	if err := restoreMemory(dir, id, messageID, cut); err != nil {
		return fmt.Errorf("rewind session memory: %w", err)
	}
	// A new file rather than a truncated one: readers see the old history or
	// the new one, and caches keyed by the file notice the change.
	if err := replaceFile(path, cut.kept); err != nil {
		return fmt.Errorf("rewind session: %w", err)
	}
	titles.Delete(path)
	// Raw output of the commands the removed runs started; the session no
	// longer refers to it.
	for _, op := range cut.dropped {
		if ValidSessionID(string(op)) {
			_ = os.RemoveAll(filepath.Join(dir, "operations", id, string(op)))
		}
	}
	// What was recorded of the removed runs goes too: the memory they started
	// from, and the snapshots of the workspace they took, which the runner
	// shows the model. Best effort: the session is rewound already, and a
	// failure here must not keep the edited prompt from running.
	_ = pruneMemory(dir, id, cut.messages)
	_ = pruneChangeRecords(dir, id, next, cut.messages)
	return nil
}

// LoadSessionBefore reads a session as RewindSession would leave it before
// the prompt with the given message ID.
func LoadSessionBefore(dir, id, messageID string) (*Transcript, error) {
	if !ValidSessionID(id) {
		return nil, fmt.Errorf("invalid session ID %q", id)
	}
	data, err := os.ReadFile(SessionPath(dir, id))
	if err != nil {
		return nil, err
	}
	kept, _, err := rewindLog(data, messageID, time.Now().UTC())
	if err != nil {
		return nil, err
	}
	return readTranscript(kept), nil
}

// logRecord is one line of a session file: the session header, an item, or
// an operation checkpoint.
type logRecord struct {
	line []byte
	item *sessionstore.Item // item records only
	// operations lists the operations a tool-call status carries, or the one
	// an operation record saves.
	operations []operation.ID
	checkpoint bool // an operation record
}

// rewindLog cuts a session log before the run that sent the prompt with the
// given message ID. It returns the log that remains and the operations only
// the removed part started (cutLog).
func rewindLog(data []byte, messageID string, now time.Time) ([]byte, []operation.ID, error) {
	cut, err := cutLog(data, messageID, now)
	return cut.kept, cut.dropped, err
}

// logCut is a session log cut before the run of a prompt (cutLog).
type logCut struct {
	kept    []byte         // the log that remains
	dropped []operation.ID // the operations only the removed part started
	// removedCalls are the arguments of the model's tool calls in what is
	// cut off, a line each.
	removedCalls []byte
	// messages are the IDs of the prompts that remain.
	messages map[string]bool
	at       time.Time // when the first record cut off was recorded
	// compactions is how many compactions the part cut off made.
	compactions int
	// shown are the notes as the conversation that remains last showed the
	// model them whole, at shownAt (notesShownIn); shownAt is zero when it
	// never did.
	shown   string
	shownAt time.Time
}

// cutLog cuts a session log before the run that sent the prompt with the
// given message ID.
//
// A run starts by recording its settings, then its prompt, so the cut falls
// before those settings: what remains is the log as the previous run left
// it. Two things are added back. Operation checkpoints the removed part wrote
// for operations started before the cut keep their latest outcome. And when
// the cut falls in history inherited from another session, a fork item marks
// what remains as inherited again, as the store requires.
func cutLog(data []byte, messageID string, now time.Time) (logCut, error) {
	// Only whole records count: the store discards a record whose write was
	// cut short.
	data = data[:bytes.LastIndexByte(data, '\n')+1]
	var records []logRecord
	for len(data) > 0 {
		n := bytes.IndexByte(data, '\n') + 1
		record, err := decodeLogRecord(data[:n])
		if err != nil {
			return logCut{}, fmt.Errorf("read session record %d: %w", len(records)+1, err)
		}
		if header := record.item == nil && !record.checkpoint; header != (len(records) == 0) {
			return logCut{}, fmt.Errorf("read session record %d: the session header must come first, and only once", len(records)+1)
		}
		records = append(records, record)
		data = data[n:]
	}
	cut := -1
	for i, record := range records {
		if input, ok := itemData[inbox.Input](record); ok && input.Kind == inbox.InputExternal && string(input.ID) == messageID {
			cut = i
			break
		}
	}
	if cut < 0 {
		return logCut{}, ErrPromptGone
	}
	for cut > 1 && isSettings(records[cut-1]) {
		cut--
	}
	result := logCut{messages: map[string]bool{}}
	if item := records[cut].item; item != nil {
		result.at = item.RecordedAt
	}
	result.removedCalls = toolCallArguments(records[cut:])

	var out bytes.Buffer
	items, sinceFork := 0, 0
	var latest session.TurnID
	started := map[operation.ID]bool{} // operations the kept log started
	owned := map[operation.ID]bool{}   // those it can still checkpoint: started after its last fork
	for _, record := range records[:cut] {
		out.Write(record.line)
		if record.item == nil {
			continue
		}
		items++
		sinceFork++
		switch data := record.item.Data.(type) {
		case session.Turn:
			latest = data.ID
		case sessionstore.Fork:
			sinceFork = 0
			clear(owned)
		case sessionstore.ToolCallStatus:
			for _, op := range record.operations {
				started[op], owned[op] = true, true
			}
		case inbox.Input:
			if data.Kind == inbox.InputExternal {
				result.messages[string(data.ID)] = true
			}
		}
		if notes, ok := notesShownIn(record); ok {
			result.shown, result.shownAt = notes, record.item.RecordedAt
		}
	}
	for _, record := range records[cut:] {
		fork, ok := itemData[sessionstore.Fork](record)
		if !ok {
			continue
		}
		if sinceFork > 0 {
			// The kept items after the last kept fork came from this fork's
			// parent: without a fork after them, the store would read them as
			// the session's own and reject their tool calls, whose operations
			// stayed with the parent.
			items++
			line, err := encodeItemRecord(sessionstore.Item{
				Sequence: sessionstore.Sequence(items), RecordedAt: now, Kind: sessionstore.ItemFork,
				Data: sessionstore.Fork{ParentID: fork.ParentID, PreviousTurnID: latest},
			})
			if err != nil {
				return logCut{}, err
			}
			out.Write(line)
			clear(owned)
		}
		break
	}
	for _, record := range records[cut:] {
		switch {
		case record.checkpoint:
			if owned[record.operations[0]] {
				out.Write(record.line)
			}
		case record.item != nil:
			for _, op := range record.operations {
				if !started[op] {
					started[op] = true
					result.dropped = append(result.dropped, op)
				}
			}
			if response, ok := record.item.Data.(sessionstore.ModelResponse); ok && response.Compacted != nil {
				result.compactions++
			}
		}
	}
	result.kept = out.Bytes()
	return result, nil
}

// toolCallArguments joins the arguments of the model's tool calls in records,
// a line each.
func toolCallArguments(records []logRecord) []byte {
	var out bytes.Buffer
	for _, record := range records {
		response, ok := itemData[sessionstore.ModelResponse](record)
		if !ok {
			continue
		}
		for _, item := range response.Response.Output {
			if call, ok := item.Data.(llm.ToolCall); ok {
				out.WriteString(call.Arguments)
				out.WriteByte('\n')
			}
		}
	}
	return out.Bytes()
}

func decodeLogRecord(line []byte) (logRecord, error) {
	var envelope struct {
		Type string         `json:"type"`
		Data jsontext.Value `json:"data"`
	}
	if err := json.Unmarshal(line, &envelope); err != nil {
		return logRecord{}, err
	}
	record := logRecord{line: line}
	switch envelope.Type {
	case "session":
	case "item":
		var value struct {
			Item       sessionstore.Item
			Operations []operation.Operation
		}
		if err := json.Unmarshal(envelope.Data, &value); err != nil {
			return logRecord{}, err
		}
		record.item = &value.Item
		for _, op := range value.Operations {
			record.operations = append(record.operations, op.ID)
		}
	case "operation":
		var value struct{ Operation operation.Operation }
		if err := json.Unmarshal(envelope.Data, &value); err != nil {
			return logRecord{}, err
		}
		record.checkpoint = true
		record.operations = []operation.ID{value.Operation.ID}
	default:
		return logRecord{}, fmt.Errorf("unsupported record type %q", envelope.Type)
	}
	return record, nil
}

// itemData returns the data of an item record of type T.
func itemData[T any](record logRecord) (T, bool) {
	var zero T
	if record.item == nil {
		return zero, false
	}
	data, ok := record.item.Data.(T)
	return data, ok
}

// isSettings reports the settings a run records before its prompt.
func isSettings(record logRecord) bool {
	input, ok := itemData[inbox.Input](record)
	if !ok || input.Kind != inbox.InputControl {
		return false
	}
	control, err := input.DecodeControlMessage()
	return err == nil && control.Mode == inbox.UpdateSettings
}

// encodeItemRecord encodes an item the way the session store writes it.
func encodeItemRecord(item sessionstore.Item) ([]byte, error) {
	encoded, err := json.Marshal(struct {
		Type string `json:"type"`
		Data any    `json:"data"`
	}{"item", struct{ Item sessionstore.Item }{item}})
	if err != nil {
		return nil, fmt.Errorf("encode session record: %w", err)
	}
	return append(encoded, '\n'), nil
}

// replaceFile atomically replaces the file at path with data.
func replaceFile(path string, data []byte) (err error) {
	dir := filepath.Dir(path)
	temp, err := os.CreateTemp(dir, ".rewind-*.tmp")
	if err != nil {
		return err
	}
	defer func() {
		if err != nil {
			os.Remove(temp.Name())
		}
	}()
	if _, err := temp.Write(data); err != nil {
		temp.Close()
		return err
	}
	if err := temp.Sync(); err != nil {
		temp.Close()
		return err
	}
	if err := temp.Close(); err != nil {
		return err
	}
	if err := os.Rename(temp.Name(), path); err != nil {
		return err
	}
	// The rename is what matters; persisting it is best effort.
	if d, err := os.Open(dir); err == nil {
		d.Sync()
		d.Close()
	}
	return nil
}
