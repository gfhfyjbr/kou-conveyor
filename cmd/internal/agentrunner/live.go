package agentrunner

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"fmt"
	"io"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"
	"unicode/utf8"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// With -stream the runner's output has, between the session items, what the
// session records only later, if at all, for front-ends to show as it comes.
// The text of a model's response as it streams:
//
//	{"type": "delta", "turn": ID, "index": n, "kind": "text"|"reasoning", "text": string}
//	{"type": "delta", "turn": ID, "reset": true}
//
// pieces of the turn's response, by the output item they are of, which a
// reset voids: the response starts over. The response the session records
// follows them, before anything else of the turn. A compaction turn's
// response, the summary, streams too; the turn's session item, before its
// pieces, says that it is one. The output of a command a tool call runs,
// while it runs:
//
//	{"type": "output", "call": ID, "operation": ID, "stream": "out"|"err", "text": string, "skipped"?: n}
//
// which the outcome of the call supersedes. Neither goes to the log.

const (
	// deltaDelay gathers the pieces of a response that stream close
	// together into one line: providers stream a few characters at a time.
	deltaDelay = 50 * time.Millisecond
	// outputPoll is how often the output of running commands is read.
	outputPoll = 200 * time.Millisecond
	// outputChunk is the most of a command's output one read gives: what
	// the command wrote before it since the read before is skipped.
	outputChunk = 16 << 10
	// maxEscape is the longest terminal escape sequence a read holds back
	// for the next to complete.
	maxEscape = 256
)

// liveOutput is what -stream writes. The observer's mutex guards it, so that
// its lines and the session items do not interleave.
type liveOutput struct {
	output io.Writer // the runner's output, without the log
	// turn is the turn whose response streams, the last the session
	// recorded. A compaction's summary streams as well: the session item of
	// its turn, written before, says what the turn is.
	turn session.TurnID
	// streamed is whether pieces of the turn's response went out since it
	// started, or last started over.
	streamed bool
	pending  []liveDelta // pieces gathered for the next line
	timer    *time.Timer // writes pending, set while there are pieces
	tails    map[operation.ID]*commandTail
	closed   bool
}

type liveDelta struct {
	Type  string         `json:"type"`
	Turn  session.TurnID `json:"turn"`
	Index int            `json:"index"`
	Kind  llm.StreamKind `json:"kind,omitempty"`
	Text  string         `json:"text,omitempty"`
	Reset bool           `json:"reset,omitempty"`
}

type liveOutputLine struct {
	Type      string       `json:"type"`
	Call      string       `json:"call"`
	Operation operation.ID `json:"operation"`
	Stream    string       `json:"stream"`
	Text      string       `json:"text"`
	Skipped   int64        `json:"skipped,omitempty"`
}

func newLiveOutput(output io.Writer) *liveOutput {
	return &liveOutput{output: output, tails: map[operation.ID]*commandTail{}}
}

// stream takes a piece of the response that streams (llm.WithStreamReporter).
func (observer *sessionObserver) stream(event llm.StreamEvent) {
	observer.mu.Lock()
	defer observer.mu.Unlock()
	live := observer.live
	if live == nil || live.closed || observer.err != nil || live.turn == "" {
		return
	}
	switch {
	case event.Reset:
		if !live.streamed {
			return
		}
		live.streamed = false
		live.pending = append(live.pending, liveDelta{Type: "delta", Turn: live.turn, Reset: true})
	case event.Text == "":
		return
	default:
		live.streamed = true
		last := len(live.pending) - 1
		if last >= 0 && !live.pending[last].Reset && live.pending[last].Turn == live.turn &&
			live.pending[last].Index == event.Index && live.pending[last].Kind == event.Kind {
			live.pending[last].Text += event.Text
		} else {
			live.pending = append(live.pending, liveDelta{
				Type: "delta", Turn: live.turn, Index: event.Index, Kind: event.Kind, Text: event.Text,
			})
		}
	}
	if live.timer == nil {
		live.timer = time.AfterFunc(deltaDelay, observer.flushDeltas)
	}
}

func (observer *sessionObserver) flushDeltas() {
	observer.mu.Lock()
	defer observer.mu.Unlock()
	observer.writeDeltas()
}

// writeDeltas writes the pieces of the response that wait. The caller holds
// the observer's mutex.
func (observer *sessionObserver) writeDeltas() {
	live := observer.live
	if live == nil {
		return
	}
	if live.timer != nil {
		live.timer.Stop()
		live.timer = nil
	}
	pending := live.pending
	live.pending = nil
	for _, delta := range pending {
		if observer.err != nil || live.closed {
			return
		}
		observer.writeLive(delta)
	}
}

// writeLive writes a line of what streams. A line that cannot be encoded is
// dropped: it only shows sooner what the session records.
func (observer *sessionObserver) writeLive(value any) {
	encoded, err := json.Marshal(value)
	if err != nil {
		return
	}
	if _, err := fmt.Fprintf(observer.live.output, "%s\n", encoded); err != nil {
		observer.fail(fmt.Errorf("write streamed output: %w", err))
	}
}

// observe follows the session: the turn whose response streams, and the
// commands whose output is read.
func (live *liveOutput) observe(item sessionstore.Item) {
	if live == nil || live.closed {
		return
	}
	switch data := item.Data.(type) {
	case session.Turn:
		live.turn, live.streamed = data.ID, false
	case sessionstore.ToolCallStatus:
		for _, value := range data.Operations {
			live.follow(data.CallID, value)
		}
	}
}

// follow starts reading the output of a command a tool call runs, and stops
// once the command has finished.
func (live *liveOutput) follow(call string, value operation.Operation) {
	tail := live.tails[value.ID]
	switch value.Status {
	case operation.StatusCompleted, operation.StatusFailed, operation.StatusCanceled:
		if tail != nil {
			tail.close()
			delete(live.tails, value.ID)
		}
		return
	}
	if tail != nil || value.Type != operation.TypeShell {
		return
	}
	// The command writes its output where the operation keeps it.
	name := string(value.ID)
	if name == "" || name == "." || name == ".." || filepath.Base(name) != name {
		return
	}
	state, err := operation.DecodeShellState(value)
	if err != nil || !filepath.IsAbs(state.BaseDirectory) {
		return
	}
	directory := filepath.Join(state.BaseDirectory, name)
	live.tails[value.ID] = &commandTail{
		call: call,
		out:  fileTail{path: filepath.Join(directory, operation.ShellOutFilename)},
		err:  fileTail{path: filepath.Join(directory, operation.ShellErrFilename)},
	}
}

// followOutput reads the output of the commands that run, until ctx ends.
func (observer *sessionObserver) followOutput(ctx context.Context) {
	ticker := time.NewTicker(outputPoll)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-ticker.C:
		}
		observer.mu.Lock()
		observer.readOutput()
		observer.mu.Unlock()
	}
}

// readOutput writes what the commands that run wrote since the last read.
// The caller holds the observer's mutex.
func (observer *sessionObserver) readOutput() {
	live := observer.live
	if live == nil || live.closed {
		return
	}
	for _, id := range slices.Sorted(maps.Keys(live.tails)) {
		tail := live.tails[id]
		for _, stream := range []struct {
			name string
			file *fileTail
		}{{"out", &tail.out}, {"err", &tail.err}} {
			text, skipped := stream.file.read()
			if text == "" && skipped == 0 {
				continue
			}
			if observer.err != nil {
				return
			}
			observer.writeLive(liveOutputLine{
				Type: "output", Call: tail.call, Operation: id, Stream: stream.name, Text: text, Skipped: skipped,
			})
		}
	}
}

// closeLive ends what streams, as the run ends.
func (observer *sessionObserver) closeLive() {
	observer.mu.Lock()
	defer observer.mu.Unlock()
	live := observer.live
	if live == nil {
		return
	}
	observer.writeDeltas()
	live.closed = true
	for id, tail := range live.tails {
		tail.close()
		delete(live.tails, id)
	}
}

// commandTail reads the output of a command as it runs.
type commandTail struct {
	call     string
	out, err fileTail
}

func (tail *commandTail) close() {
	tail.out.close()
	tail.err.close()
}

// fileTail reads what a file gains.
type fileTail struct {
	path   string
	file   *os.File
	offset int64
	// held is the end of the last read, which the next completes.
	held []byte
}

// read returns what the file gained since the last read, at most
// outputChunk bytes of it, and how many bytes before them it skipped.
func (tail *fileTail) read() (string, int64) {
	if tail.file == nil {
		file, err := os.Open(tail.path)
		if err != nil {
			return "", 0 // the command has not started yet
		}
		tail.file = file
	}
	info, err := tail.file.Stat()
	if err != nil || info.Size() <= tail.offset {
		return "", 0
	}
	start, skipped := tail.offset, int64(0)
	if info.Size()-start > outputChunk {
		start = info.Size() - outputChunk
		skipped = start - tail.offset + int64(len(tail.held))
		tail.held = nil
	}
	data := make([]byte, info.Size()-start)
	n, _ := tail.file.ReadAt(data, start)
	data = data[:n]
	tail.offset = start + int64(n)
	if skipped > 0 {
		// The read may start in the middle of a character.
		for len(data) > 0 && !utf8.RuneStart(data[0]) {
			data = data[1:]
			skipped++
		}
	}
	data = append(tail.held, data...)
	keep := unfinished(data)
	tail.held = bytes.Clone(data[len(data)-keep:])
	return strings.ToValidUTF8(string(data[:len(data)-keep]), "\uFFFD"), skipped
}

func (tail *fileTail) close() {
	if tail.file != nil {
		tail.file.Close()
		tail.file = nil
	}
}

// unfinished is how many bytes at the end of data may begin what the next
// read completes: a character cut in two, or a terminal escape sequence,
// which front-ends drop only whole.
func unfinished(data []byte) int {
	keep := 0
	for i := len(data) - 1; i >= 0 && i >= len(data)-utf8.UTFMax; i-- {
		if utf8.RuneStart(data[i]) {
			if !utf8.FullRune(data[i:]) {
				keep = len(data) - i
			}
			break
		}
	}
	if escape := bytes.LastIndexByte(data, 0x1b); escape >= 0 && len(data)-escape <= maxEscape && !escapeEnded(data[escape+1:]) {
		keep = max(keep, len(data)-escape)
	}
	return keep
}

// escapeEnded reports whether what follows an escape character completes
// the sequence it begins.
func escapeEnded(rest []byte) bool {
	if len(rest) == 0 {
		return false
	}
	switch rest[0] {
	case '[': // CSI: parameters, then a final byte
		return slices.ContainsFunc(rest[1:], func(b byte) bool { return b >= 0x40 && b <= 0x7e })
	case ']', 'P', '_', '^': // OSC and its kin end with BEL, or with ESC \, whose escape comes last
		return bytes.IndexByte(rest, 0x07) >= 0
	}
	return true
}
