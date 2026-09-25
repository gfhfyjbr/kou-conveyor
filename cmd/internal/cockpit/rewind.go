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
// runs one twice. Files the agent changed in the workspace stay as they are.
func RewindSession(dir, id, messageID string) error {
	unlock, err := LockSession(dir, id)
	if err != nil {
		return err
	}
	defer unlock()
	return rewindSession(dir, id, messageID)
}

// rewindSession is RewindSession for a caller that holds the session's lock.
func rewindSession(dir, id, messageID string) error {
	if !ValidSessionID(id) {
		return fmt.Errorf("invalid session ID %q", id)
	}
	path := SessionPath(dir, id)
	data, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	kept, dropped, err := rewindLog(data, messageID, time.Now().UTC())
	if err != nil {
		return err
	}
	// A new file rather than a truncated one: readers see the old history or
	// the new one, and caches keyed by the file notice the change.
	if err := replaceFile(path, kept); err != nil {
		return fmt.Errorf("rewind session: %w", err)
	}
	titles.Delete(path)
	// Raw output of the commands the removed runs started; the session no
	// longer refers to it.
	for _, op := range dropped {
		if ValidSessionID(string(op)) {
			_ = os.RemoveAll(filepath.Join(dir, "operations", id, string(op)))
		}
	}
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
// the removed part started.
//
// A run starts by recording its settings, then its prompt, so the cut falls
// before those settings: what remains is the log as the previous run left
// it. Two things are added back. Operation checkpoints the removed part wrote
// for operations started before the cut keep their latest outcome. And when
// the cut falls in history inherited from another session, a fork item marks
// what remains as inherited again, as the store requires.
func rewindLog(data []byte, messageID string, now time.Time) ([]byte, []operation.ID, error) {
	// Only whole records count: the store discards a record whose write was
	// cut short.
	data = data[:bytes.LastIndexByte(data, '\n')+1]
	var records []logRecord
	for len(data) > 0 {
		n := bytes.IndexByte(data, '\n') + 1
		record, err := decodeLogRecord(data[:n])
		if err != nil {
			return nil, nil, fmt.Errorf("read session record %d: %w", len(records)+1, err)
		}
		if header := record.item == nil && !record.checkpoint; header != (len(records) == 0) {
			return nil, nil, fmt.Errorf("read session record %d: the session header must come first, and only once", len(records)+1)
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
		return nil, nil, ErrPromptGone
	}
	for cut > 1 && isSettings(records[cut-1]) {
		cut--
	}

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
				return nil, nil, err
			}
			out.Write(line)
			clear(owned)
		}
		break
	}
	var dropped []operation.ID
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
					dropped = append(dropped, op)
				}
			}
		}
	}
	return out.Bytes(), dropped, nil
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
