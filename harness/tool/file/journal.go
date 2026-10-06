package file

import (
	"fmt"
	"hash/maphash"
	"sync"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

// The read journal. A model that has lost the thread reads the same lines
// again and again: one long session read a file 79 times, unchanged, its
// summaries saying the research was complete. Read counts the reads of each
// part of a file that showed the same text, and from the repeatedReads-th on
// says so after the text, for the model to keep what it needs and act on it.

// repeatedReads is the read of the same, unchanged lines that is told so.
const repeatedReads = 3

type journal struct {
	mu   sync.Mutex
	seed maphash.Seed
	// parts holds what the latest read of each part of a file showed, and
	// how many reads of it showed that; calls the count of each call, whose
	// result may be translated again.
	parts map[readPart]readCount
	calls map[string]int
}

// readPart is a part of a file a read asked for.
type readPart struct {
	path          string
	offset, limit int
}

type readCount struct {
	text  uint64
	count int
}

func newJournal() *journal {
	return &journal{seed: maphash.MakeSeed(), parts: map[readPart]readCount{}, calls: map[string]int{}}
}

// note is what a read's result says after the text when the read showed
// lines that the reads before it showed too, unchanged.
func (journal *journal) note(callID string, current operation.Operation) string {
	if current.Status != operation.StatusCompleted {
		return ""
	}
	state, err := operation.DecodeFileState(current)
	if err != nil || state.Result == nil || state.Action != operation.FileRead {
		return ""
	}
	count := journal.count(callID, readPart{path: state.Path, offset: state.Offset, limit: state.Limit}, state.Result.Text)
	if count < repeatedReads {
		return ""
	}
	return fmt.Sprintf("\n\n[harness] These lines of %s have now been read %d times, unchanged since the first. If you keep coming back to them, write down what you need from them, in your notes if you keep any, and act on it rather than read them again.", state.Path, count)
}

// count records the read of part that showed text, made by the call, and
// returns how many reads of the part showed that text.
func (journal *journal) count(callID string, part readPart, text string) int {
	journal.mu.Lock()
	defer journal.mu.Unlock()
	if count, ok := journal.calls[callID]; ok {
		return count
	}
	current := readCount{text: maphash.String(journal.seed, text), count: 1}
	if previous := journal.parts[part]; previous.text == current.text {
		current.count = previous.count + 1
	}
	journal.parts[part] = current
	journal.calls[callID] = current.count
	return current.count
}
