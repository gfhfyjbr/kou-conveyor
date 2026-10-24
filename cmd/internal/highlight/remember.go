package highlight

import (
	"container/list"
	"hash/maphash"
	"sync"
	"time"
)

// A front-end reads a session's transcript from its start again and again —
// one read again after others pushed it out of what front-ends keep, one
// rewound, the one a run goes on with — and highlighting the code its tools
// show is most of what reading it costs: chroma allocates a kilobyte and
// more for each byte it reads. The code is the same each time, so Remembered
// keeps what it highlighted by the text itself, and highlights each text
// whole once.

// remembered keeps the texts Remembered highlighted whole.
var remembered = memo{limit: rememberedLimit}

// rememberedLimit bounds what remembered keeps, in bytes: the code of a long
// session's tools takes a megabyte or two of runs.
const rememberedLimit = 16 << 20

// memoSeeds seed the two hashes a text is known by.
var memoSeeds = [2]maphash.Seed{maphash.MakeSeed(), maphash.MakeSeed()}

// Remembered highlights text, the content of the file name, until deadline
// (none if zero), as Highlight does; but a text it highlighted whole before
// comes back whole at once, whatever the deadline. The runs are shared: the
// caller must not change them.
func Remembered(name, text string, deadline time.Time) Result {
	lexer := lexerFor(name, text)
	if lexer == nil {
		return Result{Complete: true}
	}
	key := memoKey{
		lexer: lexer.Config().Name,
		size:  len(text),
		hash:  [2]uint64{maphash.String(memoSeeds[0], text), maphash.String(memoSeeds[1], text)},
	}
	if result, ok := remembered.lookup(key); ok {
		return result
	}
	result := highlightWith(lexer, text, deadline)
	if result.Complete {
		// Kept as long as they are, no longer: whoever appends to the
		// runs it shares appends to a copy of their own.
		if result.Runs != nil {
			result.Runs = append(make([]int32, 0, len(result.Runs)), result.Runs...)
		}
		remembered.store(key, result)
	}
	return result
}

// memo keeps highlighted texts up to limit bytes, letting those used least
// recently go first.
type memo struct {
	limit int

	mu    sync.Mutex
	order list.List // of *memoEntry, the most recently used first
	byKey map[memoKey]*list.Element
	size  int // of what it keeps, in bytes (memoCost)
}

// memoKey is a text, by the lexer that highlights it, its length and two
// hashes of it: that two texts share them all is as good as impossible.
type memoKey struct {
	lexer string
	size  int
	hash  [2]uint64
}

type memoEntry struct {
	key    memoKey
	result Result
}

// memoEntrySize is about what keeping a text costs besides its runs: its
// entry, its element of the order, its place in the map.
const memoEntrySize = 200

func memoCost(result Result) int { return memoEntrySize + 4*cap(result.Runs) }

func (m *memo) lookup(key memoKey) (Result, bool) {
	m.mu.Lock()
	defer m.mu.Unlock()
	element := m.byKey[key]
	if element == nil {
		return Result{}, false
	}
	m.order.MoveToFront(element)
	return element.Value.(*memoEntry).result, true
}

func (m *memo) store(key memoKey, result Result) {
	if memoCost(result) > m.limit {
		return
	}
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.byKey == nil {
		m.byKey = make(map[memoKey]*list.Element)
	}
	if m.byKey[key] != nil {
		return // highlighted twice at once: the one kept serves
	}
	m.byKey[key] = m.order.PushFront(&memoEntry{key, result})
	m.size += memoCost(result)
	for m.size > m.limit {
		oldest := m.order.Remove(m.order.Back()).(*memoEntry)
		delete(m.byKey, oldest.key)
		m.size -= memoCost(oldest.result)
	}
}
