package cockpit

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"
)

const sessionSuffix = ".session.jsonl"

// SessionInfo describes a persisted session.
type SessionInfo struct {
	ID        string    `json:"id"`
	Title     string    `json:"title"`
	UpdatedAt time.Time `json:"updated_at"`
	// PromptedAt is when the user last wrote to the session — a prompt, a
	// message forced in — which is what orders the list: the agent at work
	// does not move a session. A session without any has its UpdatedAt.
	PromptedAt time.Time `json:"prompted_at"`
	Size       int64     `json:"size"`
	Pinned     bool      `json:"pinned,omitempty"`
	// PinOrder places a pinned session among the pinned ones, by the user's
	// choosing; those pinned before there was one have none.
	PinOrder int `json:"pin_order,omitempty"`
	// Canvas and Node name the canvas and its node whose agent the session
	// is, for a session a canvas made — or adopted, one of the user's that
	// they put on a canvas.
	Canvas  string `json:"canvas,omitempty"`
	Node    string `json:"node,omitempty"`
	Adopted bool   `json:"adopted,omitempty"`
}

// ValidSessionID reports whether the runner's session store accepts id.
func ValidSessionID(id string) bool {
	if id == "" || len(id) > 128 {
		return false
	}
	for _, r := range id {
		if !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || r == '-') {
			return false
		}
	}
	return true
}

// SessionPath returns the file that holds session id.
func SessionPath(dir, id string) string { return filepath.Join(dir, id+sessionSuffix) }

// ListSessions returns the sessions persisted in dir: the pinned ones first,
// in the order the user gave them, then the others by when the user last
// wrote to them — not by when the agent last did, so a session the agent
// works in keeps its place. A missing directory has no sessions.
func ListSessions(dir string) ([]SessionInfo, error) {
	entries, err := os.ReadDir(dir)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("list sessions: %w", err)
	}
	metas := loadMetas(dir)
	sessions := make([]SessionInfo, 0, len(entries))
	for _, entry := range entries {
		id, ok := strings.CutSuffix(entry.Name(), sessionSuffix)
		if !ok || !entry.Type().IsRegular() || !ValidSessionID(id) {
			continue
		}
		info, err := entry.Info()
		if err != nil {
			continue
		}
		meta := metas[id]
		title := meta.Title
		if title == "" {
			title = sessionTitle(filepath.Join(dir, entry.Name()), info)
		}
		prompted := lastPrompt(filepath.Join(dir, entry.Name()), info)
		if prompted.IsZero() {
			prompted = info.ModTime().UTC()
		}
		sessions = append(sessions, SessionInfo{
			ID:         id,
			Title:      title,
			UpdatedAt:  info.ModTime().UTC(),
			PromptedAt: prompted,
			Size:       info.Size(),
			Pinned:     meta.Pinned,
			PinOrder:   meta.PinOrder,
			Canvas:     meta.Canvas,
			Node:       meta.Node,
			Adopted:    meta.Adopted,
		})
	}
	slices.SortFunc(sessions, func(a, b SessionInfo) int {
		if a.Pinned != b.Pinned {
			if a.Pinned {
				return -1
			}
			return 1
		}
		if a.Pinned && a.PinOrder != b.PinOrder {
			return a.PinOrder - b.PinOrder
		}
		if c := b.PromptedAt.Compare(a.PromptedAt); c != 0 {
			return c
		}
		return strings.Compare(a.ID, b.ID)
	})
	return sessions, nil
}

// When the user last wrote to a session is read from the session file,
// whichever cockpit, or the runner itself, took the prompt. The file only
// grows, so what was read of it is not read again: prompts caches, per
// file, how far it was read and the last prompt found. A file replaced — a
// rewound session — is read again from the start.
var prompts sync.Map // path → *promptScan

type promptScan struct {
	mu     sync.Mutex
	file   fs.FileInfo
	offset int64
	last   time.Time
}

// inputKind is how a user's input shows in a record; records without it are
// not decoded.
var inputKind = []byte(`"Kind":"input"`)

// lastPrompt returns when the last input from outside — the user's — was
// recorded in the session file at path, or the zero time.
func lastPrompt(path string, info fs.FileInfo) time.Time {
	value, _ := prompts.LoadOrStore(path, &promptScan{})
	scan := value.(*promptScan)
	scan.mu.Lock()
	defer scan.mu.Unlock()
	if scan.file == nil || !os.SameFile(scan.file, info) || info.Size() < scan.offset {
		scan.offset, scan.last = 0, time.Time{}
	}
	scan.file = info
	if info.Size() == scan.offset {
		return scan.last
	}
	file, err := os.Open(path)
	if err != nil {
		return scan.last
	}
	defer file.Close()
	if _, err := file.Seek(scan.offset, io.SeekStart); err != nil {
		return scan.last
	}
	// A prompt's images make its line megabytes long.
	reader := bufio.NewReaderSize(file, 64<<10)
	for {
		line, err := reader.ReadBytes('\n')
		if err != nil {
			break // the end, or a record the runner is still writing
		}
		scan.offset += int64(len(line))
		if !bytes.Contains(line, inputKind) {
			continue
		}
		var record struct {
			Type string `json:"type"`
			Data struct {
				Item struct {
					RecordedAt time.Time
					Kind       string
					Data       struct{ Kind string }
				}
			} `json:"data"`
		}
		if json.Unmarshal(line, &record) != nil || record.Type != "item" {
			continue
		}
		if item := record.Data.Item; item.Kind == "input" && item.Data.Kind == "external" && item.RecordedAt.After(scan.last) {
			scan.last = item.RecordedAt.UTC()
		}
	}
	return scan.last
}

// SessionTitle is the title a session is shown under: the user's, or its
// first prompt.
func SessionTitle(dir, id string, t *Transcript) string {
	if title := LoadMeta(dir, id).Title; title != "" {
		return title
	}
	return t.Title()
}

// ListedTitle is the title the session list shows a session under, read
// as the list does: the user's, or the first prompt at the head of its
// file.
func ListedTitle(dir, id string) string {
	if title := LoadMeta(dir, id).Title; title != "" {
		return title
	}
	return sessionTitle(SessionPath(dir, id), nil)
}

// LoadSession reads a persisted session into a transcript. A trailing record
// the runner is still writing is ignored; records that cannot be decoded are
// skipped and reported once at the end of the transcript. The file is read
// a record at a time: it runs to hundreds of megabytes, of which the
// transcript keeps little. A session read before is read on from where it
// was (loaded); the transcript is the caller's all the same.
func LoadSession(dir, id string) (*Transcript, error) {
	if !ValidSessionID(id) {
		return nil, fmt.Errorf("invalid session ID %q", id)
	}
	path := SessionPath(dir, id)
	s := loaded.take(path)
	s.mu.Lock()
	defer s.mu.Unlock()
	t, err := s.read(path)
	if err != nil {
		s.t = nil
		loaded.forget(path, s)
		return nil, err
	}
	return t, nil
}

// Reading a whole session file takes a second once it runs to a hundred
// megabytes, and front-ends read a session again and again: the one they
// show as it changes, the one a run goes on with. The file only grows while
// it is the same file, so what was read of it is not read again: loaded
// keeps, for the sessions read last, the transcript of what was read, and
// reads on from there. A file replaced — a rewound session — or written
// over is read again from the start.
var loaded = loadedSessions{byPath: make(map[string]*loadedSession)}

// loadedKept is how many sessions loaded keeps: the transcript of one runs
// to tens of megabytes.
const loadedKept = 4

type loadedSessions struct {
	mu     sync.Mutex
	byPath map[string]*loadedSession
	recent []string // the paths, the one read last at the end
}

// loadedSession is what was read of a session file.
type loadedSession struct {
	mu   sync.Mutex
	file fs.FileInfo
	// t is the transcript of the file up to offset, the end of its last
	// whole record, not settled; skipped are the records before offset
	// that could not be read. It is nil until the file is read.
	t       *Transcript
	offset  int64
	skipped int
	// tail are the bytes just before offset, which tell a file that grew
	// from one written over.
	tail []byte
	// pictures is KeepPictures as t was read.
	pictures bool
}

// loadedTail is how many bytes before where the reading stopped are
// checked before reading on.
const loadedTail = 4 << 10

// take returns what was read of the session file at path, which becomes the
// one read last; the caller locks it.
func (l *loadedSessions) take(path string) *loadedSession {
	l.mu.Lock()
	defer l.mu.Unlock()
	s := l.byPath[path]
	if s == nil {
		s = &loadedSession{}
		l.byPath[path] = s
	}
	l.recent = append(slices.DeleteFunc(l.recent, func(p string) bool { return p == path }), path)
	if over := len(l.recent) - loadedKept; over > 0 {
		for _, p := range l.recent[:over] {
			delete(l.byPath, p)
		}
		l.recent = slices.Delete(l.recent, 0, over)
	}
	return s
}

// forget drops what was read of the session file at path: s, unless what
// loaded keeps of it is no longer s; whatever it is, if s is nil.
func (l *loadedSessions) forget(path string, s *loadedSession) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if kept := l.byPath[path]; kept != nil && (s == nil || kept == s) {
		delete(l.byPath, path)
		l.recent = slices.DeleteFunc(l.recent, func(p string) bool { return p == path })
	}
}

// read reads what the session file at path gained since it was last read
// and returns a copy of its transcript, settled. The caller holds s.mu.
func (s *loadedSession) read(path string) (*Transcript, error) {
	file, err := os.Open(path)
	if err != nil {
		return nil, err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return nil, err
	}
	if !s.continued(file, info) {
		s.file, s.t, s.offset, s.skipped, s.tail, s.pictures = info, NewTranscript(), 0, 0, nil, KeepPictures
	}
	start := s.offset
	if _, err := file.Seek(start, io.SeekStart); err != nil {
		return nil, err
	}
	read, err := eachRecord(file, func(line []byte) {
		s.offset += int64(len(line)) + 1
		s.tail = keepTail(s.tail, line)
		if _, err := s.t.Apply(line); err != nil {
			s.skipped++
		}
	})
	if err != nil {
		return nil, err
	}
	t := s.t.Clone()
	t.Size = start + read
	t.settle(s.skipped)
	return t, nil
}

// continued reports whether the file is the one read, grown: the same
// file, no shorter, with the bytes before where the reading stopped those
// that were read.
func (s *loadedSession) continued(file *os.File, info fs.FileInfo) bool {
	if s.t == nil || s.pictures != KeepPictures || !os.SameFile(s.file, info) || info.Size() < s.offset {
		return false
	}
	tail := make([]byte, len(s.tail))
	_, err := file.ReadAt(tail, s.offset-int64(len(tail)))
	return err == nil && bytes.Equal(tail, s.tail)
}

// keepTail appends a record read whole, and its newline, to tail, of which
// it keeps the last loadedTail bytes.
func keepTail(tail, line []byte) []byte {
	line = line[max(0, len(line)+1-loadedTail):]
	if over := len(tail) + len(line) + 1 - loadedTail; over > 0 {
		tail = tail[:copy(tail, tail[over:])]
	}
	return append(append(tail, line...), '\n')
}

// readTranscript folds the contents of a session file into a transcript.
func readTranscript(data []byte) *Transcript {
	t := NewTranscript()
	t.Size = int64(len(data))
	if end := bytes.LastIndexByte(data, '\n'); end >= 0 {
		data = data[:end]
	} else {
		data = nil
	}
	skipped := 0
	for line := range bytes.SplitSeq(data, []byte{'\n'}) {
		if _, err := t.Apply(line); err != nil {
			skipped++
		}
	}
	t.settle(skipped)
	return t
}

// settle ends the reading of a session file: records that could not be read
// are told of, and nothing is going on.
func (t *Transcript) settle(skipped int) {
	if skipped > 0 {
		t.put(&Entry{
			ID: "skipped", Kind: KindNotice,
			Text: fmt.Sprintf("%d session records could not be read", skipped),
		})
	}
	t.Activity = ""
}

// eachRecord calls fn with each line of r, without its newline, and returns
// how many bytes it read; a last line without one, a record the runner is
// still writing, is left out. A line is fn's only until fn returns.
func eachRecord(r io.Reader, fn func(line []byte)) (int64, error) {
	reader := bufio.NewReaderSize(r, 256<<10)
	var long []byte // a line longer than the reader's buffer: a prompt's images, a picture
	var n int64
	for {
		chunk, err := reader.ReadSlice('\n')
		n += int64(len(chunk))
		switch {
		case errors.Is(err, bufio.ErrBufferFull):
			long = append(long, chunk...)
			continue
		case errors.Is(err, io.EOF):
			return n, nil
		case err != nil:
			return n, err
		}
		line := chunk
		if len(long) > 0 {
			long = append(long, chunk...)
			line = long
		}
		fn(line[:len(line)-1])
		long = long[:0]
	}
}

// A session's first prompt is persisted before its first turn and stays
// while the file does; rewinding a session replaces its file. So titles are
// cached per file: path → cachedTitle.
var titles sync.Map

type cachedTitle struct {
	title string
	file  fs.FileInfo
}

// sessionTitle reads the first prompt from the head of a session file; info
// describes the file, or is nil to look it up.
func sessionTitle(path string, info fs.FileInfo) string {
	if info == nil {
		var err error
		if info, err = os.Stat(path); err != nil {
			return ""
		}
	}
	if cached, ok := titles.Load(path); ok && os.SameFile(cached.(cachedTitle).file, info) {
		return cached.(cachedTitle).title
	}
	file, err := os.Open(path)
	if err != nil {
		return ""
	}
	defer file.Close()
	// A prompt's images make its line megabytes long.
	reader := bufio.NewReaderSize(file, 64<<10)
	for lines := 0; lines < 64; lines++ {
		line, err := reader.ReadBytes('\n')
		if err != nil {
			break // the end, or a record the runner is still writing
		}
		var record struct {
			Type string `json:"type"`
			Data struct {
				Item struct {
					Kind string
					Data struct {
						Kind    string
						Payload jsontext.Value
					}
				}
			} `json:"data"`
		}
		if json.Unmarshal(line, &record) != nil || record.Type != "item" {
			continue
		}
		item := record.Data.Item
		switch {
		case item.Kind == "input" && item.Data.Kind == "external":
			title := Headline(payloadText(item.Data.Payload), 80)
			if title != "" {
				titles.Store(path, cachedTitle{title, info})
			}
			return title
		case item.Kind != "input":
			return ""
		}
	}
	return ""
}
