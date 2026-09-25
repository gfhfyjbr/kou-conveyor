package cockpit

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"maps"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
)

// Meta is what the cockpits add to a session the runner owns: a title of the
// user's choosing and a pin, with the pinned session's place among the
// pinned ones. It lives beside the session store, which never reads it.
type Meta struct {
	Title    string `json:"title,omitzero"`
	Pinned   bool   `json:"pinned,omitzero"`
	PinOrder int    `json:"pin_order,omitzero"`
}

func metaPath(dir, id string) string { return filepath.Join(dir, ".meta", id+".json") }

// LoadMeta returns a session's metadata; a session without any has none.
// Titles are cleaned on the way in as well as out: the file may come from
// anywhere the sessions directory does.
func LoadMeta(dir, id string) Meta {
	var m Meta
	if data, err := os.ReadFile(metaPath(dir, id)); err == nil {
		_ = json.Unmarshal(data, &m)
	}
	m.Title = Headline(Clean(m.Title), 120)
	return m
}

// loadMetas reads the metadata of every session that has any.
func loadMetas(dir string) map[string]Meta {
	metas := map[string]Meta{}
	entries, _ := os.ReadDir(filepath.Join(dir, ".meta"))
	for _, entry := range entries {
		if id, ok := strings.CutSuffix(entry.Name(), ".json"); ok && ValidSessionID(id) {
			metas[id] = LoadMeta(dir, id)
		}
	}
	return metas
}

func saveMeta(dir, id string, m Meta) error {
	path := metaPath(dir, id)
	if m == (Meta{}) {
		if err := os.Remove(path); err != nil && !errors.Is(err, fs.ErrNotExist) {
			return err
		}
		return nil
	}
	data, err := json.Marshal(m)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	temp := fmt.Sprintf("%s.%d.tmp", path, time.Now().UnixNano())
	if err := os.WriteFile(temp, data, 0o600); err != nil {
		return err
	}
	if err := os.Rename(temp, path); err != nil {
		os.Remove(temp)
		return err
	}
	return nil
}

func updateMeta(dir, id string, change func(*Meta)) error {
	if !ValidSessionID(id) {
		return fmt.Errorf("invalid session ID %q", id)
	}
	if _, err := os.Stat(SessionPath(dir, id)); err != nil {
		return err
	}
	m := LoadMeta(dir, id)
	change(&m)
	if err := saveMeta(dir, id, m); err != nil {
		return fmt.Errorf("update session: %w", err)
	}
	return nil
}

// RenameSession titles a session; an empty title brings back its first prompt.
func RenameSession(dir, id, title string) error {
	return updateMeta(dir, id, func(m *Meta) { m.Title = Headline(Clean(title), 120) })
}

// PinSession keeps a session at the top of the list: a session pinned goes
// after those pinned before it, and keeps its place until the user moves
// it (OrderPins).
func PinSession(dir, id string, pinned bool) error {
	last := 0
	if pinned {
		for other, meta := range loadMetas(dir) {
			if other != id && meta.Pinned {
				last = max(last, meta.PinOrder)
			}
		}
	}
	return updateMeta(dir, id, func(m *Meta) {
		if pinned && !m.Pinned {
			m.PinOrder = last + 1
		}
		if !pinned {
			m.PinOrder = 0
		}
		m.Pinned = pinned
	})
}

// OrderPins puts the pinned sessions in the order ids gives them; pinned
// sessions ids leaves out go after, as they were. Sessions not pinned are
// left alone.
func OrderPins(dir string, ids []string) error {
	metas := loadMetas(dir)
	placed := map[string]bool{}
	order := 0
	var problems []error
	place := func(id string) {
		if placed[id] || !metas[id].Pinned {
			return
		}
		placed[id] = true
		order++
		if metas[id].PinOrder == order {
			return
		}
		if err := updateMeta(dir, id, func(m *Meta) { m.PinOrder = order }); err != nil && !errors.Is(err, fs.ErrNotExist) {
			problems = append(problems, err)
		}
	}
	for _, id := range ids {
		if ValidSessionID(id) {
			place(id)
		}
	}
	// The rest keep their order among themselves, after those placed.
	rest := slices.Collect(maps.Keys(metas))
	slices.SortFunc(rest, func(a, b string) int {
		return metas[a].PinOrder - metas[b].PinOrder
	})
	for _, id := range rest {
		place(id)
	}
	return errors.Join(problems...)
}

// DeleteSession removes a session no run holds, with the output its tools
// left in the runner's operations directory. The lock is held while the files
// go, so no run can start on a half-deleted session.
func DeleteSession(dir, id string) error {
	unlock, err := LockSession(dir, id)
	if err != nil {
		return err
	}
	defer unlock()
	path := SessionPath(dir, id)
	if err := os.Remove(path); err != nil {
		return err
	}
	titles.Delete(path)
	if err := os.Remove(metaPath(dir, id)); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("delete session metadata: %w", err)
	}
	// Tool results live in the session file itself; these are the raw
	// streams they were read from.
	if err := os.RemoveAll(filepath.Join(dir, "operations", id)); err != nil {
		return fmt.Errorf("delete tool output: %w", err)
	}
	// What its runs changed in the workspace goes with it (see Changes).
	if err := os.Remove(filepath.Join(dir, ".changes", id+".jsonl")); err != nil && !errors.Is(err, fs.ErrNotExist) {
		return fmt.Errorf("delete change records: %w", err)
	}
	return nil
}

// FindSession resolves a session ID or an unambiguous prefix of one.
func FindSession(dir, prefix string) (string, error) {
	prefix = strings.TrimSpace(prefix)
	if !ValidSessionID(prefix) {
		return "", fmt.Errorf("invalid session ID %q", prefix)
	}
	if _, err := os.Stat(SessionPath(dir, prefix)); err == nil {
		return prefix, nil
	}
	list, err := ListSessions(dir)
	if err != nil {
		return "", err
	}
	var found []string
	for _, info := range list {
		if strings.HasPrefix(info.ID, prefix) {
			found = append(found, info.ID)
		}
	}
	switch len(found) {
	case 0:
		return "", fmt.Errorf("no session starts with %q: %w", prefix, fs.ErrNotExist)
	case 1:
		return found[0], nil
	}
	return "", fmt.Errorf("%d sessions start with %q; type more of the ID", len(found), prefix)
}

// Branch is a new session made from part of another.
type Branch struct {
	// SessionID is empty when nothing came before the prompt: the branch is
	// then a new session that starts with it.
	SessionID string `json:"session_id,omitzero"`
	// Prompt is the text of the prompt the branch was made before, to edit
	// and send again.
	Prompt string `json:"prompt,omitzero"`
	// Images are the images that prompt brought; a front-end that edits it
	// gets their bytes, a browser what they are (PromptImages has them).
	Images []Image `json:"-"`
}

// ErrBranchRunning reports a branch requested while the session runs.
var ErrBranchRunning = errors.New("stop the run before branching the session")

// BranchSession copies a session's history up to, not including, the prompt
// with the given message ID, or all of it when messageID is empty, into a new
// session. History is copied whole turns at a time, as the session store
// forks at turn boundaries.
func BranchSession(ctx context.Context, dir, id, messageID string) (Branch, error) {
	if !ValidSessionID(id) {
		return Branch{}, fmt.Errorf("invalid session ID %q", id)
	}
	if SessionBusy(dir, id) {
		return Branch{}, ErrBranchRunning
	}
	store, err := localfile.New(dir)
	if err != nil {
		return Branch{}, err
	}
	var branch Branch
	var latest, before session.TurnID
	found := false
	after := sessionstore.BeforeFirst
scan:
	for {
		page, err := store.Items(ctx, session.ID(id), after, 512)
		if err != nil {
			return Branch{}, fmt.Errorf("read session: %w", err)
		}
		for _, item := range page.Items {
			switch data := item.Data.(type) {
			case session.Turn:
				latest = data.ID
			case inbox.Input:
				if messageID != "" && data.Kind == inbox.InputExternal && string(data.ID) == messageID {
					found, before, branch.Prompt = true, latest, payloadText(data.Payload)
					branch.Images = promptImages(data.Payload)
					break scan
				}
			}
		}
		if !page.More || page.NextAfter <= after {
			break
		}
		after = page.NextAfter
	}
	switch {
	case messageID == "":
		before = latest
	case !found:
		return Branch{}, fmt.Errorf("prompt %s is not in session %s: %w", messageID, id, fs.ErrNotExist)
	}
	if before == "" {
		return branch, nil
	}
	branch.SessionID = uuid.New().String()
	if _, err := store.Fork(ctx, session.ID(branch.SessionID), session.ID(id), before); err != nil {
		return Branch{}, fmt.Errorf("branch session: %w", err)
	}
	title := LoadMeta(dir, id).Title
	if title == "" {
		title = sessionTitle(SessionPath(dir, id), nil)
	}
	if title != "" {
		_ = saveMeta(dir, branch.SessionID, Meta{Title: Headline(title, 110) + " · branch"})
	}
	return branch, nil
}

// ContinuePrompt asks the agent to pick up an interrupted run.
const ContinuePrompt = "Continue where you left off."

// Interrupted reports a transcript whose last run did not finish: the last
// prompt was never answered, tools were left running, or the agent stopped
// between tool calls without a final answer.
func (t *Transcript) Interrupted() bool {
	if len(t.active) > 0 {
		return true
	}
	for i := len(t.Entries) - 1; i >= 0; i-- {
		switch e := t.Entries[i]; e.Kind {
		case KindAssistant:
			return false
		case KindUser, KindTool:
			return true
		}
	}
	return false
}

// ExportMarkdown renders a transcript as a Markdown document.
func ExportMarkdown(title, id string, t *Transcript, now time.Time) string {
	var b strings.Builder
	if title == "" {
		title = "Untitled session"
	}
	fmt.Fprintf(&b, "# %s\n\n", strings.ReplaceAll(title, "\n", " "))
	fmt.Fprintf(&b, "_kou-conveyor session `%s`, exported %s_\n", id, now.Local().Format("2006-01-02 15:04"))
	prompt := 0
	for _, e := range t.Entries {
		stamp := ""
		if !e.At.IsZero() {
			stamp = " · " + e.At.Local().Format("15:04")
		}
		switch e.Kind {
		case KindUser:
			prompt++
			if e.Model != "" {
				stamp += " · " + inlineCode(e.Model)
			}
			fmt.Fprintf(&b, "\n---\n\n## %d. You%s\n\n%s\n", prompt, stamp, strings.TrimSpace(e.Text))
		case KindAssistant:
			fmt.Fprintf(&b, "\n**Agent**%s\n\n%s\n", stamp, strings.TrimSpace(e.Text))
		case KindReasoning:
			fmt.Fprintf(&b, "\n<details><summary>Thinking</summary>\n\n%s\n\n</details>\n", strings.TrimSpace(e.Text))
		case KindTool:
			tool := e.Tool
			fmt.Fprintf(&b, "\n**%s**", orName(tool.Name))
			if input := strings.TrimSpace(tool.Input); input != "" && !strings.Contains(input, "\n") {
				fmt.Fprintf(&b, " %s", inlineCode(input))
			}
			fmt.Fprintf(&b, " — %s", tool.State)
			if tool.ExitCode != nil {
				fmt.Fprintf(&b, ", exit %d", *tool.ExitCode)
			}
			b.WriteString("\n")
			if input := strings.TrimSpace(tool.Input); strings.Contains(input, "\n") {
				b.WriteString("\n" + fence(input, ""))
			}
			for _, part := range []struct{ label, text string }{{"", tool.Output}, {"stderr", tool.Stderr}, {"error", tool.Error}} {
				if text := strings.TrimRight(part.text, "\n"); strings.TrimSpace(text) != "" {
					if part.label != "" {
						fmt.Fprintf(&b, "\n_%s_\n", part.label)
					}
					b.WriteString("\n" + fence(clipOutput(text, 400), "text"))
				}
			}
		case KindNotice:
			fmt.Fprintf(&b, "\n> %s\n", strings.ReplaceAll(strings.TrimSpace(e.Text), "\n", "\n> "))
			if detail := strings.TrimSpace(e.Detail); detail != "" {
				label := "Details"
				if strings.HasPrefix(e.ID, "compaction:") {
					label = "Summary"
				}
				fmt.Fprintf(&b, "\n<details><summary>%s</summary>\n\n%s\n\n</details>\n", label, detail)
			}
		case KindError:
			fmt.Fprintf(&b, "\n> **Error:** %s\n", strings.ReplaceAll(strings.TrimSpace(e.Text), "\n", "\n> "))
		}
	}
	return b.String()
}

func orName(name string) string {
	if name == "" {
		return "Tool"
	}
	return name
}

// inlineCode quotes text in a code span that its own backticks cannot close.
func inlineCode(text string) string {
	ticks := strings.Repeat("`", longestRun(text, '`')+1)
	if strings.HasPrefix(text, "`") || strings.HasSuffix(text, "`") {
		return ticks + " " + text + " " + ticks
	}
	return ticks + text + ticks
}

// fence quotes text in a code block that its own backticks cannot close.
func fence(text, info string) string {
	ticks := strings.Repeat("`", max(3, longestRun(text, '`')+1))
	return ticks + info + "\n" + text + "\n" + ticks + "\n"
}

func longestRun(text string, r rune) int {
	longest, run := 0, 0
	for _, c := range text {
		if c == r {
			run++
			longest = max(longest, run)
		} else {
			run = 0
		}
	}
	return longest
}

func clipOutput(text string, limit int) string {
	lines := strings.Split(text, "\n")
	if len(lines) <= limit {
		return text
	}
	head := lines[:limit/2]
	tail := lines[len(lines)-limit/2:]
	return strings.Join(head, "\n") + fmt.Sprintf("\n… %d lines omitted …\n", len(lines)-len(head)-len(tail)) + strings.Join(tail, "\n")
}

// ExportName is a file name for a session's Markdown export.
func ExportName(title, id string) string {
	var b strings.Builder
	for _, r := range strings.ToLower(title) {
		switch {
		case r >= 'a' && r <= 'z', r >= '0' && r <= '9':
			b.WriteRune(r)
		case b.Len() > 0 && !strings.HasSuffix(b.String(), "-"):
			b.WriteByte('-')
		}
		if b.Len() >= 48 {
			break
		}
	}
	name := strings.Trim(b.String(), "-")
	if name == "" {
		name = "session-" + id[:min(8, len(id))]
	}
	return name + ".md"
}
