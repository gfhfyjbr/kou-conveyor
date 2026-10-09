package contextbuilder

import (
	"encoding/json/v2"
	"fmt"
	"slices"
	"strings"

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
	// summary, up to notesLimit bytes.
	Notes     string
	ReadNotes func() (string, error)
	// Summaries is the file that keeps the summaries of the session's
	// compactions, oldest first; the message points to it.
	Summaries string
}

const (
	// notesLimit bounds what the message shows of the notes.
	notesLimit = 16_000
	// touchedLimit bounds the files the message lists.
	touchedLimit = 20
)

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
// conversation says after the summary: the notes, the files the
// conversation touched, and where to find what was compacted away.
func (current *builder) memoryText() string {
	var text strings.Builder
	memory := current.memory
	if memory.Notes != "" {
		notes := ""
		if memory.ReadNotes != nil {
			if read, err := memory.ReadNotes(); err == nil {
				notes = strings.TrimSpace(read)
			}
		}
		if notes == "" {
			fmt.Fprintf(&text, "\n\nYou keep notes across compactions in %s, which is empty so far. Write there what the summary may lose and you must not: the hypotheses you are testing, the evidence for and against them, the negative results, what you deferred and the next step. Each compaction shows the notes here.", memory.Notes)
		} else {
			fmt.Fprintf(&text, "\n\nYour notes, which you keep in %s across compactions; keep them current, since no compaction changes them:\n<notes>\n%s\n</notes>", memory.Notes, clip(notes, notesLimit))
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
	details := "If you need details from before the compaction, such as exact code, error messages, command output or what you wrote, "
	switch {
	case current.offers(tool.TranscriptSearchName):
		pointers = append(pointers, details+"search the session's transcript with "+tool.TranscriptSearchName+": it finds them in the whole conversation, compacted parts included.")
	case current.offers(tool.CodeName):
		pointers = append(pointers, details+"search the session's transcript with transcriptSearch() in "+tool.CodeName+": it finds them in the whole conversation, compacted parts included.")
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
