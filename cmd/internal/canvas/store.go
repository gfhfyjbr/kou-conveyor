package canvas

import (
	"bufio"
	"bytes"
	"crypto/rand"
	"encoding/hex"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// A workspace keeps its canvases in .harness/canvases: <id>.json is a
// canvas's document, and <id>/ what goes with it — messages.jsonl, the
// journal of its messages (rotated to messages.jsonl.1 past 5 MiB),
// pending.jsonl, those that wait, messages/<message>.txt the whole text of
// those too long to deliver whole, and state/<node>/ what its sources keep.

const (
	// journalLimit is how large the journal grows before it is rotated.
	journalLimit = 5 << 20
	// docLimit bounds a document read from disk.
	docLimit = 16 << 20
)

// Directory is where a workspace keeps its canvases.
func Directory(workspace string) string {
	return filepath.Join(workspace, ".harness", "canvases")
}

func docPath(workspace, id string) string { return filepath.Join(Directory(workspace), id+".json") }

func dataDir(workspace, id string) string { return filepath.Join(Directory(workspace), id) }

// stateDir is where a source node keeps what it remembers.
func stateDir(workspace, id, node string) string {
	return filepath.Join(dataDir(workspace, id), "state", node)
}

// loadDoc reads a canvas's document. readOnly is set for one a newer
// version wrote, which this one shows but does not change.
func loadDoc(workspace, id string) (doc *Doc, readOnly bool, err error) {
	if !ValidID(id) {
		return nil, false, fs.ErrNotExist
	}
	file, err := os.Open(docPath(workspace, id))
	if err != nil {
		return nil, false, err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return nil, false, err
	}
	if info.Size() > docLimit {
		return nil, false, fmt.Errorf("canvas %s: the document is over %d MiB", id, docLimit>>20)
	}
	var d Doc
	if err := json.UnmarshalRead(file, &d); err != nil {
		return nil, false, fmt.Errorf("canvas %s: %w", id, err)
	}
	readOnly = d.Version > Version
	if d.Version == 0 {
		d.Version = Version
	}
	d.ID = id
	d.normalize()
	return &d, readOnly, nil
}

// saveDoc writes a canvas's document, whole or not at all.
func saveDoc(workspace string, d *Doc) error {
	data, err := json.Marshal(d, jsontext.WithIndent("  "))
	if err != nil {
		return err
	}
	return writeAtomic(docPath(workspace, d.ID), append(data, '\n'), 0o600)
}

// writeAtomic writes a file by renaming a complete copy over it.
func writeAtomic(path string, data []byte, perm os.FileMode) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	file, err := os.CreateTemp(dir, "."+filepath.Base(path)+".*")
	if err != nil {
		return err
	}
	name := file.Name()
	_, err = file.Write(data)
	if err == nil {
		err = file.Chmod(perm)
	}
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Rename(name, path)
	}
	if err != nil {
		os.Remove(name)
	}
	return err
}

// listIDs lists the canvases a workspace has.
func listIDs(workspace string) []string {
	entries, err := os.ReadDir(Directory(workspace))
	if err != nil {
		return nil
	}
	var ids []string
	for _, entry := range entries {
		if id, ok := strings.CutSuffix(entry.Name(), ".json"); ok && !entry.IsDir() && ValidID(id) {
			ids = append(ids, id)
		}
	}
	return ids
}

// removeCanvasFiles removes a canvas's document and what goes with it.
func removeCanvasFiles(workspace, id string) error {
	if !ValidID(id) {
		return fs.ErrNotExist
	}
	err := os.Remove(docPath(workspace, id))
	if errors.Is(err, fs.ErrNotExist) {
		err = nil
	}
	if rmErr := os.RemoveAll(dataDir(workspace, id)); err == nil {
		err = rmErr
	}
	return err
}

// appendJournal adds messages, as they are now, to the canvas's journal.
func appendJournal(dir string, messages ...*Message) error {
	if len(messages) == 0 {
		return nil
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	path := filepath.Join(dir, "messages.jsonl")
	if info, err := os.Stat(path); err == nil && info.Size() > journalLimit {
		_ = os.Rename(path, path+".1")
	}
	var buf bytes.Buffer
	for _, m := range messages {
		data, err := json.Marshal(m)
		if err != nil {
			return err
		}
		buf.Write(data)
		buf.WriteByte('\n')
	}
	file, err := os.OpenFile(path, os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	_, err = file.Write(buf.Bytes())
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	return err
}

// readJournal reads the last n messages of the canvas's journal, each as it
// was last written, the oldest first.
func readJournal(dir string, n int) []*Message {
	var order []string
	byID := map[string]*Message{}
	for _, name := range []string{"messages.jsonl.1", "messages.jsonl"} {
		file, err := os.Open(filepath.Join(dir, name))
		if err != nil {
			continue
		}
		scanner := bufio.NewScanner(file)
		scanner.Buffer(make([]byte, 64<<10), 4<<20)
		for scanner.Scan() {
			var m Message
			if json.Unmarshal(scanner.Bytes(), &m) != nil || m.ID == "" {
				continue
			}
			if _, seen := byID[m.ID]; !seen {
				order = append(order, m.ID)
			}
			byID[m.ID] = &m
		}
		file.Close()
	}
	if len(order) > n {
		order = order[len(order)-n:]
	}
	out := make([]*Message, 0, len(order))
	for _, id := range order {
		out = append(out, byID[id])
	}
	return out
}

// savePending writes the messages that wait.
func savePending(dir string, messages []*Message) error {
	path := filepath.Join(dir, "pending.jsonl")
	if len(messages) == 0 {
		err := os.Remove(path)
		if errors.Is(err, fs.ErrNotExist) {
			err = nil
		}
		return err
	}
	var buf bytes.Buffer
	for _, m := range messages {
		data, err := json.Marshal(m)
		if err != nil {
			return err
		}
		buf.Write(data)
		buf.WriteByte('\n')
	}
	return writeAtomic(path, buf.Bytes(), 0o600)
}

// loadPending reads the messages that waited when the canvas was last
// saved.
func loadPending(dir string) []*Message {
	data, err := os.ReadFile(filepath.Join(dir, "pending.jsonl"))
	if err != nil {
		return nil
	}
	var out []*Message
	for line := range bytes.SplitSeq(data, []byte{'\n'}) {
		var m Message
		if len(bytes.TrimSpace(line)) == 0 || json.Unmarshal(line, &m) != nil || m.ID == "" {
			continue
		}
		switch m.State {
		case MessagePending, MessageAwaiting:
		case messageDelivering:
			// The server stopped while it was delivered: whether it was is
			// not known, so it goes again.
			m.State = MessagePending
		default:
			continue
		}
		out = append(out, &m)
	}
	return out
}

// saveLong keeps the whole text of a message too long to deliver whole, and
// returns where.
func saveLong(dir, id, text string) (string, error) {
	path := filepath.Join(dir, "messages", id+".txt")
	if err := writeAtomic(path, []byte(text), 0o600); err != nil {
		return "", err
	}
	return path, nil
}

// LoadSecret reads the key the canvases' tokens are signed with, making it
// the first time.
func LoadSecret(path string) ([]byte, error) {
	data, err := os.ReadFile(path)
	if err == nil {
		if secret, err := hex.DecodeString(strings.TrimSpace(string(data))); err == nil && len(secret) >= 32 {
			return secret, nil
		}
	} else if !errors.Is(err, fs.ErrNotExist) {
		return nil, err
	}
	secret := make([]byte, 32)
	if _, err := rand.Read(secret); err != nil {
		return nil, err
	}
	if err := writeAtomic(path, []byte(hex.EncodeToString(secret)+"\n"), 0o600); err != nil {
		return nil, err
	}
	return secret, nil
}

// sortedKeys lists a map's keys in order.
func sortedKeys[V any](m map[string]V) []string {
	keys := make([]string, 0, len(m))
	for key := range m {
		keys = append(keys, key)
	}
	slices.Sort(keys)
	return keys
}
