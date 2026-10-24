package cockpit

import (
	"bytes"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// A session's notes and the summaries of its compactions are the model's own,
// kept beside the session log (see the runner's session memory): <id>.notes.md
// and <id>.summaries.md. Unlike the log, a rewind does not cut them, so a
// rewound run would otherwise read what its discarded future wrote: the
// harness shows the model its notes as a run starts when they changed since
// the conversation last showed them, and every compaction shows them and
// points at the summaries. Snapshots record, as each prompt's run begins, the
// notes and the length of the summaries it starts from, so rewinding to that
// prompt restores them to how they were before the run. They live in
// <id>.memory.jsonl, one record per run, in order. Without a snapshot (a
// session made before they were kept, or a prompt sent into the run of
// another) a rewind restores what the log that remains tells (notesBefore,
// cutSummaries).

const (
	memorySuffix = ".memory.jsonl"
	notesSuffix  = ".notes.md"
)

func memoryPath(dir, id string) string    { return filepath.Join(dir, id+memorySuffix) }
func notesPath(dir, id string) string     { return filepath.Join(dir, id+notesSuffix) }
func summariesPath(dir, id string) string { return filepath.Join(dir, id+".summaries.md") }
func changeRecordsPath(dir, id string) string {
	return filepath.Join(dir, ".changes", id+".jsonl")
}

// memorySnapshot is one line of a session's memory snapshots: the notes and the
// length of the summaries a prompt's run started from.
type memorySnapshot struct {
	Message   string    `json:"message"`
	Notes     string    `json:"notes,omitzero"`
	Summaries int64     `json:"summaries,omitzero"`
	At        time.Time `json:"at"`
}

// snapshotMemory records the notes and summaries the run of the prompt with the
// given message ID starts from, so a later rewind to it can restore them. It is
// best effort and appends: a repeated run of one prompt (a resume) adds another
// record, and restoreMemory takes the first, which is the state before the
// prompt first ran.
func snapshotMemory(dir, id, messageID string) error {
	if !ValidSessionID(id) || messageID == "" {
		return nil
	}
	notes, err := os.ReadFile(notesPath(dir, id))
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		return err
	}
	var size int64
	switch info, err := os.Stat(summariesPath(dir, id)); {
	case err == nil:
		size = info.Size()
	case !errors.Is(err, fs.ErrNotExist):
		return err
	}
	data, err := json.Marshal(memorySnapshot{
		Message: messageID, Notes: string(notes), Summaries: size, At: time.Now().UTC(),
	})
	if err != nil {
		return err
	}
	file, err := os.OpenFile(memoryPath(dir, id), os.O_CREATE|os.O_APPEND|os.O_WRONLY, 0o600)
	if err != nil {
		return err
	}
	_, err = file.Write(append(data, '\n'))
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	return err
}

// restoreMemory takes a session's notes and summaries back to how they were
// before the run of the prompt with the given message ID, which cut cut off
// the session's log: to the snapshot the run recorded as it started, else as
// far as what remains of the log tells.
func restoreMemory(dir, id, messageID string, cut logCut) error {
	snaps, err := readMemorySnapshots(dir, id)
	if err != nil {
		return err
	}
	notes, summaries := notesPath(dir, id), summariesPath(dir, id)
	for _, snap := range snaps {
		if snap.Message != messageID {
			continue
		}
		if err := writeNotes(notes, snap.Notes); err != nil {
			return fmt.Errorf("restore notes: %w", err)
		}
		// The summaries only ever grow, so the run and those after it
		// appended past the snapshot: cut back to the length it recorded.
		if err := truncateFile(summaries, snap.Summaries); err != nil {
			return fmt.Errorf("restore summaries: %w", err)
		}
		return nil
	}
	if err := cutSummaries(summaries, cut.at, cut.compactions); err != nil {
		return fmt.Errorf("restore summaries: %w", err)
	}
	if before, ok := notesBefore(id, snaps, cut); ok {
		if err := writeNotes(notes, before); err != nil {
			return fmt.Errorf("restore notes: %w", err)
		}
	}
	return nil
}

// notesBefore tells what a session's notes were where cut cut its log, with no
// snapshot taken there; false when they are as they were then.
//
// The model changes its notes with tool calls, which name the file: when no
// call cut off names it, the notes are as they were. Else they go back to the
// latest state what remains tells of: the notes a run that remains started
// from, or what the conversation last showed of them, when it showed them
// whole; and to none when it tells of none, as when nothing that remains
// wrote them. What the remaining runs wrote after that is lost, rather than
// the rewound run read what the discarded ones wrote.
func notesBefore(id string, snaps []memorySnapshot, cut logCut) (string, bool) {
	if !cut.notesNamed {
		return "", false
	}
	notes, at := "", time.Time{}
	for _, snap := range snaps {
		if cut.messages[snap.Message] && snap.At.Before(cut.at) && !snap.At.Before(at) {
			notes, at = snap.Notes, snap.At
		}
	}
	if !cut.shownAt.IsZero() && cut.shownAt.After(at) {
		// What a message shows of the notes is trimmed (ShownNotes).
		notes = cut.shown
		if notes != "" {
			notes += "\n"
		}
	}
	return notes, true
}

// notesShownIn reads the notes an item record of a session log showed the
// model whole: the message of a compaction (sessionstore.Compacted), or one of
// the harness that they changed (contextbuilder.NotesUpdate). Notes too long
// to show whole tell only their beginning and end, and do not count.
func notesShownIn(record logRecord) (string, bool) {
	var notes string
	switch data := record.item.Data.(type) {
	case sessionstore.ModelResponse:
		if data.Compacted == nil || data.Compacted.Notes == nil {
			return "", false
		}
		notes = *data.Compacted.Notes
	case inbox.Input:
		if data.Kind != inbox.InputControl {
			return "", false
		}
		control, err := data.DecodeControlMessage()
		if err != nil || control.Mode != inbox.Heartbeat {
			return "", false
		}
		shown, ok := contextbuilder.NotesShownBy(control.Reason)
		if !ok {
			return "", false
		}
		notes = shown
	default:
		return "", false
	}
	return notes, contextbuilder.ShowsWholeNotes(notes)
}

// writeNotes makes the notes file hold notes, removing it for none.
func writeNotes(path, notes string) error {
	current, err := os.ReadFile(path)
	switch {
	case err == nil && string(current) == notes:
		return nil
	case notes == "":
		if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
			return err
		}
		return nil
	}
	return replaceFile(path, []byte(notes))
}

// cutSummaries removes from a session's summaries those of the compactions a
// rewind cut off the log at the given time: the sections the runner appended
// for them (sessionMemory.compacted), the last of the file. Their headers tell
// the time to the second: the sections of a later second than the cut go,
// those of an earlier one stay, and of those of its second, as many go from
// the end as the part cut off made compactions.
func cutSummaries(path string, at time.Time, compactions int) error {
	if at.IsZero() {
		return nil
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, fs.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	at = at.Truncate(time.Second)
	type section struct {
		start int
		at    time.Time
	}
	var sections []section
	const header = "## Compaction at "
	for from := 0; ; {
		i := bytes.Index(data[from:], []byte(header))
		if i < 0 {
			break
		}
		i += from
		from = i + len(header)
		// A section starts the file, or the line after the blank one that
		// ends the section before.
		if i > 0 && !bytes.HasSuffix(data[:i], []byte("\n\n")) {
			continue
		}
		stamp, _, _ := strings.Cut(string(data[from:min(len(data), from+64)]), "\n")
		stamp, _, _ = strings.Cut(stamp, ",")
		if t, err := time.Parse(time.RFC3339, stamp); err == nil {
			sections = append(sections, section{i, t})
		}
	}
	later := len(sections) // the first of a later second than the cut
	for i, s := range sections {
		if s.at.After(at) {
			later = i
			break
		}
	}
	same := later // the first of the cut's second
	for same > 0 && sections[same-1].at.Equal(at) {
		same--
	}
	cut := max(same, min(later, len(sections)-compactions))
	if cut == len(sections) {
		return nil
	}
	return truncateFile(path, int64(sections[cut].start))
}

// truncateFile cuts the file to size, removing it when size is zero. A file
// already that short or shorter, or gone, is left alone.
func truncateFile(path string, size int64) error {
	if size <= 0 {
		if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
			return err
		}
		return nil
	}
	info, err := os.Stat(path)
	if errors.Is(err, fs.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	if info.Size() <= size {
		return nil
	}
	return os.Truncate(path, size)
}

// pruneMemory drops the snapshots of the runs a rewind removed, keeping those
// whose prompt remains in the session.
func pruneMemory(dir, id string, keep map[string]bool) error {
	snaps, err := readMemorySnapshots(dir, id)
	if err != nil || len(snaps) == 0 {
		return err
	}
	kept := snaps[:0]
	for _, snap := range snaps {
		if keep[snap.Message] {
			kept = append(kept, snap)
		}
	}
	if len(kept) == len(snaps) {
		return nil
	}
	path := memoryPath(dir, id)
	if len(kept) == 0 {
		if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
			return err
		}
		return nil
	}
	var out bytes.Buffer
	for _, snap := range kept {
		data, err := json.Marshal(snap)
		if err != nil {
			return err
		}
		out.Write(data)
		out.WriteByte('\n')
	}
	return replaceFile(path, out.Bytes())
}

// readMemorySnapshots reads a session's memory snapshots, skipping a line that
// is none, as one whose write was cut short. The notes in a line can be long:
// no limit on it keeps a rewind from failing on them.
func readMemorySnapshots(dir, id string) ([]memorySnapshot, error) {
	data, err := os.ReadFile(memoryPath(dir, id))
	if errors.Is(err, fs.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, err
	}
	var snaps []memorySnapshot
	for line := range bytes.Lines(data) {
		var snap memorySnapshot
		if line = bytes.TrimSpace(line); len(line) == 0 || json.Unmarshal(line, &snap) != nil {
			continue
		}
		snaps = append(snaps, snap)
	}
	return snaps, nil
}

// forgetMemory removes a deleted session's memory snapshots.
func forgetMemory(dir, id string) {
	if ValidSessionID(id) {
		_ = os.Remove(memoryPath(dir, id))
	}
}

// pruneChangeRecords drops the workspace change records of the prompts a
// rewind removed, keeping those whose prompt remains: the runner tells the
// model of the snapshot the latest run started from, which must not be one
// the discarded future took.
//
// next is the message ID of the prompt whose run the rewind makes way for, ""
// for none. The cockpits take the snapshot a run starts from before they start
// it (Changes.Begin), and so before the rewind: the records at the end, from
// the latest taken as next's run started, are the coming run's, and stay.
func pruneChangeRecords(dir, id, next string, keep map[string]bool) error {
	path := changeRecordsPath(dir, id)
	data, err := os.ReadFile(path)
	if errors.Is(err, fs.ErrNotExist) {
		return nil
	}
	if err != nil {
		return err
	}
	type line struct {
		text   []byte
		record changeRecord
		read   bool // the line is a record
	}
	var lines []line
	for _, text := range bytes.Split(data, []byte{'\n'}) {
		if text = bytes.TrimSpace(text); len(text) == 0 {
			continue
		}
		l := line{text: text}
		l.read = json.Unmarshal(text, &l.record) == nil
		lines = append(lines, l)
	}
	coming := len(lines)
	for i := len(lines) - 1; next != "" && i >= 0 && lines[i].read && lines[i].record.Message == next; i-- {
		if lines[i].record.Before {
			coming = i
			break
		}
	}
	var out bytes.Buffer
	changed := false
	for i, l := range lines {
		if i < coming && l.read && !keep[l.record.Message] {
			changed = true
			continue
		}
		out.Write(l.text)
		out.WriteByte('\n')
	}
	switch {
	case !changed:
		return nil
	case out.Len() == 0:
		if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
			return err
		}
		return nil
	}
	return replaceFile(path, out.Bytes())
}
