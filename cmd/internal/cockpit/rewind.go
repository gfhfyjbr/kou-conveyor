package cockpit

import (
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
)

// Editing a prompt that was already sent rewinds its session: the session
// file goes back to how it was before the prompt's run began, and the edited
// prompt runs from there under the same session ID. What remains is a state
// the session really was in, so the runner resumes it like any other.
//
// A session file runs to hundreds of megabytes, so it is read a record at a
// time, and what remains of it is copied from it, not held: a rewind holds
// where the cut falls and the little that is added back (cutLog).

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
	file, err := os.Open(path)
	if err != nil {
		return err
	}
	// The file is copied from until it is replaced; it stays readable after.
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return err
	}
	cut, ok := cuts.take(path, messageID, info)
	if !ok {
		if cut, err = cutLog(io.NewSectionReader(file, 0, info.Size()), id, messageID); err != nil {
			return err
		}
	}
	remains, err := cut.remains(file, time.Now().UTC())
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
	temp, err := writeTemp(filepath.Dir(path), func(w *os.File) error {
		// Hidden from io.CopyBuffer, the file's ReadFrom and the reader's
		// WriteTo would copy 32 KB at a time.
		_, err := io.CopyBuffer(struct{ io.Writer }{w}, struct{ io.Reader }{remains}, make([]byte, copyBuffer))
		return err
	})
	// What remains is copied: the old file closes before it is replaced,
	// which a system may refuse to do to a file that is open.
	file.Close()
	if err == nil {
		err = renameTemp(temp, path)
	}
	if err != nil {
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
// the prompt with the given message ID. A rewind of the same prompt that
// follows, while the file is as it was, cuts it where this found the cut.
func LoadSessionBefore(dir, id, messageID string) (*Transcript, error) {
	if !ValidSessionID(id) {
		return nil, fmt.Errorf("invalid session ID %q", id)
	}
	path := SessionPath(dir, id)
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return nil, err
	}
	cut, err := cutLog(io.NewSectionReader(file, 0, info.Size()), id, messageID)
	if err != nil {
		return nil, err
	}
	cuts.put(path, messageID, info, cut)
	remains, err := cut.remains(file, time.Now().UTC())
	if err != nil {
		return nil, err
	}
	return readTranscript(remains)
}

// Editing a prompt reads the session as the rewind would leave it, to show
// it, then rewinds it (LoadSessionBefore, then RewindSession): cuts keeps the
// cut the first found for the second, which takes it while the file is the
// one that was read, as long as it was.
var cuts lastCut

type lastCut struct {
	mu        sync.Mutex
	path      string
	messageID string
	file      fs.FileInfo // nil when there is none
	cut       logCut
}

func (l *lastCut) put(path, messageID string, file fs.FileInfo, cut logCut) {
	l.mu.Lock()
	defer l.mu.Unlock()
	l.path, l.messageID, l.file, l.cut = path, messageID, file, cut
}

// take returns the cut put for the prompt of the file at path, if file is
// that file as it was, and forgets it.
func (l *lastCut) take(path, messageID string, file fs.FileInfo) (logCut, bool) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.file == nil || l.path != path || l.messageID != messageID || !os.SameFile(l.file, file) ||
		l.file.Size() != file.Size() || !l.file.ModTime().Equal(file.ModTime()) {
		return logCut{}, false
	}
	cut := l.cut
	l.path, l.messageID, l.file, l.cut = "", "", nil, logCut{}
	return cut, true
}

// logRecord is one record of a session file, decoded: the session header,
// an item, or an operation checkpoint.
type logRecord struct {
	item *sessionstore.Item // item records only
	// operations lists the operations a tool-call status carries, or the one
	// an operation record saves.
	operations []operation.ID
	checkpoint bool // an operation record
}

// logCut is a session log cut before the run of a prompt (cutLog).
type logCut struct {
	// keep is how long the start of the log that remains is: the records
	// before the cut.
	keep int64
	// What is added back after them (remains): when fork is not nil, the
	// fork item, the sequence-th item of the log; else the checkpoints at
	// the places of the log listed.
	fork        *sessionstore.Fork
	sequence    sessionstore.Sequence
	checkpoints []span
	dropped     []operation.ID // the operations only the removed part started
	// notesNamed reports whether one of the model's tool calls cut off names
	// the session's notes file (notesBefore).
	notesNamed bool
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

// span is where a record is in a session log, its newline included.
type span struct{ at, size int64 }

// remains returns the log that remains of log, which c cut: the records
// before the cut, read from log, then what is added back, made as of now.
func (c logCut) remains(log io.ReaderAt, now time.Time) (io.Reader, error) {
	parts := []io.Reader{io.NewSectionReader(log, 0, c.keep)}
	if c.fork != nil {
		line, err := encodeItemRecord(sessionstore.Item{
			Sequence: c.sequence, RecordedAt: now, Kind: sessionstore.ItemFork, Data: *c.fork,
		})
		if err != nil {
			return nil, err
		}
		parts = append(parts, bytes.NewReader(line))
	}
	for _, s := range c.checkpoints {
		parts = append(parts, io.NewSectionReader(log, s.at, s.size))
	}
	return io.MultiReader(parts...), nil
}

// cutLog cuts the log of session id before the run that sent the prompt with
// the given message ID, reading it a record at a time.
//
// A run starts by recording its settings, then its prompt, so the cut falls
// before those settings: what remains is the log as the previous run left
// it. Two things are added back. Operation checkpoints the removed part wrote
// for operations started before the cut keep their latest outcome. And when
// the cut falls in history inherited from another session, a fork item marks
// what remains as inherited again, as the store requires.
func cutLog(log io.Reader, id, messageID string) (logCut, error) {
	c := logCutter{
		messageID: messageID,
		notesFile: id + notesSuffix,
		cut:       logCut{messages: map[string]bool{}},
		started:   map[operation.ID]bool{},
		owned:     map[operation.ID]bool{},
	}
	// Only whole records count: the store discards a record whose write was
	// cut short (eachRecord).
	var failed error
	var at int64
	_, err := eachRecord(log, func(line []byte) {
		size := int64(len(line)) + 1
		if failed == nil {
			failed = c.add(line, span{at, size})
		}
		at += size
	})
	switch {
	case failed != nil:
		return logCut{}, failed
	case err != nil:
		return logCut{}, err
	case !c.found:
		return logCut{}, ErrPromptGone
	}
	if c.fork != nil && c.sinceFork > 0 {
		// The kept items after the last kept fork came from this fork's
		// parent: without a fork after them, the store would read them as
		// the session's own and reject their tool calls, whose operations
		// stayed with the parent. Nothing kept is the session's own to
		// checkpoint then.
		c.cut.fork = &sessionstore.Fork{ParentID: c.fork.ParentID, PreviousTurnID: c.latest}
		c.cut.sequence = sessionstore.Sequence(c.items + 1)
		c.cut.checkpoints = nil
	}
	return c.cut, nil
}

// logCutter cuts a session log as cutLog reads it.
type logCutter struct {
	messageID string
	notesFile string // the name of the session's notes file
	records   int    // read so far
	// found is set once the prompt is read: the records from the cut on are
	// cut off.
	found bool
	// pending are the settings read last, which remain unless the prompt
	// comes next.
	pending []pendingRecord
	cut     logCut

	items, sinceFork int // of the records that remain
	latest           session.TurnID
	started          map[operation.ID]bool // operations the kept log started
	owned            map[operation.ID]bool // those it can still checkpoint: started after its last fork
	fork             *sessionstore.Fork    // the first fork cut off
}

type pendingRecord struct {
	record logRecord
	at     span
}

// add reads the next record of the log, the line at where.
func (c *logCutter) add(line []byte, where span) error {
	record, err := decodeLogRecord(line)
	if err != nil {
		return fmt.Errorf("read session record %d: %w", c.records+1, err)
	}
	if header := record.item == nil && !record.checkpoint; header != (c.records == 0) {
		return fmt.Errorf("read session record %d: the session header must come first, and only once", c.records+1)
	}
	c.records++
	switch {
	case c.found:
		c.cutOff(record, where)
	case isPrompt(record, c.messageID):
		c.found = true
		first := pendingRecord{record, where}
		if len(c.pending) > 0 {
			first = c.pending[0]
		}
		c.cut.keep = first.at.at
		if first.record.item != nil {
			c.cut.at = first.record.item.RecordedAt
		}
		for _, p := range c.pending {
			c.cutOff(p.record, p.at)
		}
		c.pending = nil
		c.cutOff(record, where)
	case isSettings(record):
		c.pending = append(c.pending, pendingRecord{record, where})
	default:
		for _, p := range c.pending {
			c.remain(p.record)
		}
		c.pending = c.pending[:0]
		c.remain(record)
	}
	return nil
}

// remain takes in a record before the cut.
func (c *logCutter) remain(record logRecord) {
	if record.item == nil {
		return
	}
	c.items++
	c.sinceFork++
	switch data := record.item.Data.(type) {
	case session.Turn:
		c.latest = data.ID
	case sessionstore.Fork:
		c.sinceFork = 0
		clear(c.owned)
	case sessionstore.ToolCallStatus:
		for _, op := range record.operations {
			c.started[op], c.owned[op] = true, true
		}
	case inbox.Input:
		if data.Kind == inbox.InputExternal {
			c.cut.messages[string(data.ID)] = true
		}
	}
	if notes, ok := notesShownIn(record); ok {
		c.cut.shown, c.cut.shownAt = notes, record.item.RecordedAt
	}
}

// cutOff takes in a record from the cut on, the line at where.
func (c *logCutter) cutOff(record logRecord, where span) {
	switch {
	case record.checkpoint:
		if c.owned[record.operations[0]] {
			c.cut.checkpoints = append(c.cut.checkpoints, where)
		}
	case record.item != nil:
		for _, op := range record.operations {
			if !c.started[op] {
				c.started[op] = true
				c.cut.dropped = append(c.cut.dropped, op)
			}
		}
		switch data := record.item.Data.(type) {
		case sessionstore.ModelResponse:
			if data.Compacted != nil {
				c.cut.compactions++
			}
			for _, item := range data.Response.Output {
				if call, ok := item.Data.(llm.ToolCall); ok && strings.Contains(call.Arguments, c.notesFile) {
					c.cut.notesNamed = true
				}
			}
		case sessionstore.Fork:
			if c.fork == nil {
				c.fork = &data
			}
		}
	}
}

// isPrompt reports the prompt with the given message ID.
func isPrompt(record logRecord, messageID string) bool {
	input, ok := itemData[inbox.Input](record)
	return ok && input.Kind == inbox.InputExternal && string(input.ID) == messageID
}

func decodeLogRecord(line []byte) (logRecord, error) {
	var envelope struct {
		Type string         `json:"type"`
		Data jsontext.Value `json:"data"`
	}
	if err := json.Unmarshal(line, &envelope); err != nil {
		return logRecord{}, err
	}
	var record logRecord
	switch envelope.Type {
	case "session":
	case "item":
		var value struct {
			Item       sessionstore.Item
			Operations []localfile.ItemOperation
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

// copyBuffer is how much a rewind copies of a session file at a time: what
// remains of it runs to hundreds of megabytes.
const copyBuffer = 1 << 20

// replaceFile atomically replaces the file at path with data.
func replaceFile(path string, data []byte) error {
	temp, err := writeTemp(filepath.Dir(path), func(w *os.File) error {
		_, err := w.Write(data)
		return err
	})
	if err != nil {
		return err
	}
	return renameTemp(temp, path)
}

// writeTemp writes a new file in dir with write, to replace a file of dir
// with (renameTemp), and returns its path.
func writeTemp(dir string, write func(*os.File) error) (string, error) {
	temp, err := os.CreateTemp(dir, ".rewind-*.tmp")
	if err != nil {
		return "", err
	}
	err = write(temp)
	if err == nil {
		err = temp.Sync()
	}
	if closeErr := temp.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		os.Remove(temp.Name())
		return "", err
	}
	return temp.Name(), nil
}

// renameTemp atomically replaces the file at path with the one writeTemp
// wrote at temp.
func renameTemp(temp, path string) error {
	if err := os.Rename(temp, path); err != nil {
		os.Remove(temp)
		return err
	}
	// The rename is what matters; persisting it is best effort.
	if d, err := os.Open(filepath.Dir(path)); err == nil {
		d.Sync()
		d.Close()
	}
	return nil
}
