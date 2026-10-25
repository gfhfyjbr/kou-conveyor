package contextbuilder

import (
	"encoding/json/v2"
	"fmt"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

// Memory. A summary keeps what its writer thought mattered, and every
// compaction summarizes the summary before it, so what one compaction drops
// no later one gets back. The message that stands in for a compacted
// conversation therefore carries more than the summary: the notes the model
// keeps for itself in a file, which no compaction touches; the files the
// conversation read and changed; and where to find what was compacted away.

// Memory says where what a compaction keeps besides the summary is.
type Memory struct {
	// Notes is the file the model keeps notes in across compactions, and
	// ReadNotes reads it; nil reads nothing. What it holds follows the
	// summary, up to NotesLimit bytes (ShownNotes).
	Notes     string
	ReadNotes func() (string, error)
	// Summaries is the file that keeps the summaries of the session's
	// compactions, oldest first; the message points to it.
	Summaries string
}

const (
	// NotesLimit bounds what a message shows of the notes: notes longer
	// show their beginning and end, and a mark between says how much of
	// the middle is left out and where the whole file is.
	NotesLimit = 32_000
	// touchedLimit bounds the files the message lists.
	touchedLimit = 20
)

// NotesUpdatePrefix starts the message that shows the notes again when they
// changed since the conversation last showed them (NotesUpdate).
const NotesUpdatePrefix = "[harness] Your notes changed since the conversation last showed them."

// Compacted is the message that stands in for a compacted conversation, as
// the run that compacted it made it: kept with the compaction and given to
// Compact again, it is shown as it was by every run that restores the
// session, rather than made anew from notes that changed since, which would
// change the start of every request and lose the provider's cache of the
// conversation.
type Compacted struct {
	// Message is the text of the message.
	Message string
	// Notes is what the message shows of the notes, "" when they were
	// empty; nil when it shows none.
	Notes *string
}

// ShownNotes is what a message shows of notes: all of them up to NotesLimit
// bytes; past it, their beginning and end, cut at lines where they can be,
// with a mark between that says how much is left out and where the whole
// file, path, is.
func ShownNotes(notes, path string) string {
	notes = strings.TrimSpace(notes)
	if len(notes) <= NotesLimit {
		return notes
	}
	head, tail := NotesLimit*2/3, len(notes)-NotesLimit/3
	// A cut falls at the end of a line within a few lines of the limit.
	if at := strings.LastIndexByte(notes[:head], '\n'); at > head-2000 {
		head = at
	}
	if at := strings.IndexByte(notes[tail:], '\n'); at >= 0 && at < 2000 {
		tail += at + 1
	}
	for head > 0 && !utf8.RuneStart(notes[head]) {
		head--
	}
	for tail < len(notes) && !utf8.RuneStart(notes[tail]) {
		tail++
	}
	return notes[:head] + fmt.Sprintf("\n[… %s "+notesLeftOut+", of %s in all: the whole file is %s …]\n", kilobytes(tail-head), kilobytes(len(notes)), path) + notes[tail:]
}

// notesLeftOut marks, in what ShownNotes shows of notes too long to show
// whole, where their middle is left out.
const notesLeftOut = "from the middle of the notes are left out here"

// ShowsWholeNotes reports whether notes as ShownNotes showed them are the
// whole notes, rather than their beginning and end.
func ShowsWholeNotes(shown string) bool {
	return !strings.Contains(shown, notesLeftOut)
}

// NotesShownBy reads the notes a message of NotesUpdate shows; false for any
// other message. What a session's conversation last showed of the notes is
// the latest of these or of a compaction's (Compacted.Notes).
func NotesShownBy(message string) (string, bool) {
	return notesIn(message)
}

// kilobytes says a size in kilobytes, the way a person would.
func kilobytes(size int) string {
	if size < 10_000 {
		return fmt.Sprintf("%.1f KB", float64(size)/1000)
	}
	return fmt.Sprintf("%d KB", (size+500)/1000)
}

// readNotes is what a message would show of the notes now; nil when the
// session keeps none.
func (current *builder) readNotes() *string {
	memory := current.memory
	if memory.Notes == "" {
		return nil
	}
	notes := ""
	if memory.ReadNotes != nil {
		if read, err := memory.ReadNotes(); err == nil {
			notes = ShownNotes(read, memory.Notes)
		}
	}
	return &notes
}

// NotesUpdate is a message that shows the notes as they are now, when they
// changed since the conversation last showed them: a run that restores a
// compacted conversation shows its compaction as it was, notes included,
// and this tells the model what changed since. It reports false when
// nothing did, or no compaction has shown the notes yet.
func (current *builder) NotesUpdate() (string, bool) {
	if current.shownNotes == nil {
		return "", false
	}
	notes := current.readNotes()
	if notes == nil || *notes == *current.shownNotes {
		return "", false
	}
	if *notes == "" {
		return NotesUpdatePrefix + " The file " + current.memory.Notes + " is empty now.", true
	}
	return NotesUpdatePrefix + " They are in " + current.memory.Notes + ", and read now:\n<notes>\n" + *notes + "\n</notes>", true
}

// notesIn reads the notes a message of NotesUpdate shows; false for any
// other message.
func notesIn(message string) (string, bool) {
	if !strings.HasPrefix(message, NotesUpdatePrefix) {
		return "", false
	}
	start := strings.Index(message, "\n<notes>\n")
	end := strings.LastIndex(message, "\n</notes>")
	if start < 0 || end < start+len("\n<notes>\n") {
		return "", true
	}
	return message[start+len("\n<notes>\n") : end], true
}

// SetMemory says where what a compaction keeps besides the summary is.
func (current *builder) SetMemory(memory Memory) {
	current.memory = memory
}

// touchedFile is a file the conversation read or changed.
type touchedFile struct {
	path    string
	read    bool
	changed bool
}

// touchedFiles are the files the calls of items read or changed through
// the file tools, by any name a gateway gave them, the latest first, before
// those of earlier, already listed. A call that failed touched nothing.
func touchedFiles(items []llm.Item, earlier []touchedFile) []touchedFile {
	failed := make(map[string]bool)
	for _, item := range items {
		if result, ok := item.Data.(llm.ToolResult); ok && len(result.Output) != 0 &&
			strings.HasPrefix(result.Output[0].Value, "Error:") {
			failed[result.CallID] = true
		}
	}
	var files []touchedFile
	touch := func(path string, changed bool) {
		path = strings.TrimSpace(path)
		if path == "" {
			return
		}
		at := slices.IndexFunc(files, func(file touchedFile) bool { return file.path == path })
		if at < 0 {
			files = append(files, touchedFile{path: path})
			at = len(files) - 1
		}
		// files is oldest first until it is reversed: a touch moves the
		// file to the end.
		file := files[at]
		files = append(slices.Delete(files, at, at+1), file)
		last := &files[len(files)-1]
		last.read = last.read || !changed
		last.changed = last.changed || changed
	}
	names := []string{tool.ReadName, tool.EditName, tool.WriteName, tool.ApplyPatchName}
	for _, item := range items {
		call, ok := item.Data.(llm.ToolCall)
		if !ok || item.Type != llm.ItemToolCall || failed[call.CallID] {
			continue
		}
		name := call.Name
		if !slices.Contains(names, name) {
			aliases := tool.Aliases(name, names)
			if len(aliases) != 1 {
				continue
			}
			name = aliases[0]
		}
		var arguments struct {
			Path     string `json:"path"`
			FilePath string `json:"file_path"`
			Input    string `json:"input"`
			Patch    string `json:"patch"`
		}
		if json.Unmarshal([]byte(call.Arguments), &arguments) != nil {
			continue
		}
		path := arguments.Path
		if path == "" {
			path = arguments.FilePath
		}
		switch name {
		case tool.ReadName:
			touch(path, false)
		case tool.EditName, tool.WriteName:
			touch(path, true)
		case tool.ApplyPatchName:
			patch := arguments.Input
			if strings.TrimSpace(patch) == "" {
				patch = arguments.Patch
			}
			for line := range strings.Lines(patch) {
				for _, marker := range []string{"*** Add File: ", "*** Update File: ", "*** Delete File: ", "*** Move to: "} {
					if rest, found := strings.CutPrefix(strings.TrimSpace(line), marker); found {
						touch(rest, true)
					}
				}
			}
		}
	}
	slices.Reverse(files)
	for _, file := range earlier {
		if len(files) >= touchedLimit {
			break
		}
		if !slices.ContainsFunc(files, func(listed touchedFile) bool { return listed.path == file.path }) {
			files = append(files, file)
		}
	}
	return files[:min(len(files), touchedLimit)]
}

// memoryText is what the message that stands in for a compacted
// conversation says after the summary: the notes, as readNotes reads them,
// the files the conversation touched, and where to find what was compacted
// away.
func (current *builder) memoryText(notes *string) string {
	var text strings.Builder
	memory := current.memory
	if notes != nil {
		if *notes == "" {
			fmt.Fprintf(&text, "\n\nYou keep notes across compactions in %s, which is empty so far. Write there what the summary may lose and you must not: the hypotheses you are testing, the evidence for and against them, the negative results, what you deferred and the next step. Each compaction shows the notes here, up to %s; past that, only their beginning and end.", memory.Notes, kilobytes(NotesLimit))
		} else {
			fmt.Fprintf(&text, "\n\nYour notes, which you keep in %s across compactions; keep them current, since no compaction changes them:\n<notes>\n%s\n</notes>", memory.Notes, *notes)
		}
	}
	if len(current.touched) != 0 {
		text.WriteString("\n\nFiles read or changed before the compaction, the latest first; read a file again before you change it:\n<files>\n")
		for _, file := range current.touched {
			var how []string
			if file.read {
				how = append(how, "read")
			}
			if file.changed {
				how = append(how, "changed")
			}
			fmt.Fprintf(&text, "- %s (%s)\n", file.path, strings.Join(how, ", "))
		}
		text.WriteString("</files>")
	}
	var pointers []string
	details := "For anything exact the summary does not hold — code, an error message, a command's output, your own wording — "
	switch {
	case current.offers(tool.TranscriptSearchName):
		pointers = append(pointers, details+"search the session's transcript with "+tool.TranscriptSearchName+": it covers the whole conversation, compacted parts included.")
	case current.offers(tool.CodeName):
		pointers = append(pointers, details+"search the session's transcript with transcriptSearch() in "+tool.CodeName+": it covers the whole conversation, compacted parts included.")
	case current.transcript != "":
		pointers = append(pointers, details+"search the session's transcript at "+current.transcript+" with rg; it is JSON Lines, one session record per line, too large to read whole.")
	}
	if memory.Summaries != "" {
		pointers = append(pointers, "The summaries of this session's compactions, oldest first, are in "+memory.Summaries+".")
	}
	if len(pointers) != 0 {
		text.WriteString("\n\n" + strings.Join(pointers, " "))
	}
	return text.String()
}

// offers reports a tool the model is offered.
func (current *builder) offers(name string) bool {
	return slices.ContainsFunc(current.request.Tools, func(offered llm.Tool) bool { return offered.Name == name })
}
