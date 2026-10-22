package cockpit

import (
	"bytes"
	"encoding/json/v2"
	"fmt"
	"maps"
	"slices"
	"strings"
	"time"
	"unicode"

	"github.com/charmbracelet/x/ansi"

	"github.com/gfhfyjbr/kou-conveyor/harness/session"
)

// What a runner started with Request.Stream writes besides the session
// items, which Apply ignores, Live folds: the text of the model's response
// as it streams, and the output of commands as they run. Neither makes
// entries. The text that streams goes to a preview, which front-ends show
// as an entry with the preview's ID until the response is recorded: then
// the entry it became replaces it (Entry.Replaces), and a preview left over
// goes (Entry.Gone). What streams of a compaction's summary is no entry's:
// its pieces say so (Delta.Compaction), for front-ends to show them apart
// from the transcript while the compaction goes, and its previews go once
// the compaction's notice comes. The output of a command goes to its tool
// call's entry, until the call's outcome supersedes it.

// Delta is text that streamed of a model's response.
type Delta struct {
	ID   string    `json:"id"`   // the preview's
	Kind string    `json:"kind"` // KindAssistant or KindReasoning
	At   time.Time `json:"at"`   // when the preview began
	Text string    `json:"text"` // what it gains
	// Compaction says the text is of a compaction's summary, which no entry
	// replaces: the compaction's notice holds the summary once it is done.
	Compaction bool `json:"compaction,omitzero"`
}

// Output is what a command a tool call runs wrote, as it runs.
type Output struct {
	ID     string `json:"id"`     // the tool call's entry
	Stream string `json:"stream"` // "out" or "err"
	Text   string `json:"text"`
	// Skipped is how much the command wrote before Text that is not shown,
	// in bytes: it wrote faster than it is read.
	Skipped int64 `json:"skipped,omitempty"`
}

// Streamed is what a line Live folds makes.
type Streamed struct {
	Delta  *Delta
	Output *Output
	// Changed are previews a response that started over leaves void.
	Changed []*Entry
}

// preview is the text that streams of an output item of a turn's response.
type preview struct {
	id    string
	kind  string
	index int
	at    time.Time
}

// Live folds a line of what the runner streams. ok is false for a line that
// is none, which is for Apply.
func (t *Transcript) Live(line []byte) (streamed Streamed, ok bool) {
	var envelope struct {
		Type string `json:"type"`
		// delta
		Turn  session.TurnID `json:"turn"`
		Index int            `json:"index"`
		Kind  string         `json:"kind"`
		Reset bool           `json:"reset"`
		// output
		Call    string `json:"call"`
		Stream  string `json:"stream"`
		Skipped int64  `json:"skipped"`
		// both
		Text string `json:"text"`
	}
	line = bytes.TrimSpace(line)
	if !bytes.HasPrefix(line, []byte(`{"type":"delta"`)) && !bytes.HasPrefix(line, []byte(`{"type":"output"`)) {
		return Streamed{}, false
	}
	if json.Unmarshal(line, &envelope) != nil {
		return Streamed{}, false
	}
	switch envelope.Type {
	case "delta":
		if envelope.Turn == "" {
			return Streamed{}, true
		}
		if envelope.Reset {
			return Streamed{Changed: t.voidTurn(envelope.Turn)}, true
		}
		kind := KindAssistant
		switch envelope.Kind {
		case "text":
		case "reasoning":
			kind = KindReasoning
		default:
			return Streamed{}, true
		}
		text := cleanStream(envelope.Text)
		if text == "" {
			return Streamed{}, true
		}
		p := t.preview(envelope.Turn, envelope.Index, kind)
		return Streamed{Delta: &Delta{ID: p.id, Kind: p.kind, At: p.at, Text: text, Compaction: t.compaction[envelope.Turn]}}, true
	case "output":
		id := "tool:" + envelope.Call
		// The outcome of a call supersedes what it wrote as it ran.
		if e := t.index[id]; e == nil || e.Tool == nil || e.Tool.Terminal() {
			return Streamed{}, true
		}
		stream := "out"
		if envelope.Stream == "err" {
			stream = "err"
		}
		text := cleanStream(envelope.Text)
		if text == "" && envelope.Skipped <= 0 {
			return Streamed{}, true
		}
		return Streamed{Output: &Output{ID: id, Stream: stream, Text: text, Skipped: max(envelope.Skipped, 0)}}, true
	}
	return Streamed{}, false
}

// preview returns the preview of an output item of a turn's response,
// which begins with the item's first text.
func (t *Transcript) preview(turn session.TurnID, index int, kind string) *preview {
	for _, p := range t.previews[turn] {
		if p.index == index && p.kind == kind {
			return p
		}
	}
	t.streamed++
	p := &preview{id: fmt.Sprintf("stream:%s.%d", turn, t.streamed), kind: kind, index: index, at: time.Now().UTC()}
	if t.previews == nil {
		t.previews = make(map[session.TurnID][]*preview)
	}
	t.previews[turn] = append(t.previews[turn], p)
	return p
}

// replacePreviews gives the entries a turn's response made the previews they
// replace, the first of each kind to the first entry of the kind and so on,
// and returns the previews no entry replaces, void.
func (t *Transcript) replacePreviews(turn session.TurnID, entries []*Entry) []*Entry {
	previews := t.previews[turn]
	delete(t.previews, turn)
	for _, e := range entries {
		for i, p := range previews {
			if p != nil && p.kind == e.Kind {
				e.Replaces, previews[i] = p.id, nil
				break
			}
		}
	}
	var gone []*Entry
	for _, p := range previews {
		if p != nil {
			gone = append(gone, &Entry{ID: p.id, Kind: p.kind, At: p.at, Gone: true})
		}
	}
	return gone
}

// voidTurn voids what streamed of a turn's response, which starts over.
func (t *Transcript) voidTurn(turn session.TurnID) []*Entry {
	return t.replacePreviews(turn, nil)
}

// voidPreviews voids what streamed of the responses of turns other than
// except, which are over.
func (t *Transcript) voidPreviews(except session.TurnID) []*Entry {
	var gone []*Entry
	for _, turn := range slices.Sorted(maps.Keys(t.previews)) {
		if turn != except {
			gone = append(gone, t.voidTurn(turn)...)
		}
	}
	return gone
}

// cleanStream is Clean for text that comes in pieces: a carriage return
// stays, for front-ends to rewrite the line it is on with what follows,
// which may come in the next piece.
func cleanStream(s string) string {
	s = ansi.Strip(strings.ToValidUTF8(s, "\uFFFD"))
	return strings.Map(func(r rune) rune {
		if r == '\n' || r == '\t' || r == '\r' {
			return r
		}
		if unicode.IsControl(r) {
			return -1
		}
		return r
	}, s)
}
