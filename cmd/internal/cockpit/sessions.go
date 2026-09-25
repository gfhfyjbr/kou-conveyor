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

// LoadSession reads a persisted session into a transcript. A trailing record
// the runner is still writing is ignored; records that cannot be decoded are
// skipped and reported once at the end of the transcript.
func LoadSession(dir, id string) (*Transcript, error) {
	if !ValidSessionID(id) {
		return nil, fmt.Errorf("invalid session ID %q", id)
	}
	data, err := os.ReadFile(SessionPath(dir, id))
	if err != nil {
		return nil, err
	}
	return readTranscript(data), nil
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
	if skipped > 0 {
		t.put(&Entry{
			ID: "skipped", Kind: KindNotice,
			Text: fmt.Sprintf("%d session records could not be read", skipped),
		})
	}
	t.Activity = ""
	return t
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
