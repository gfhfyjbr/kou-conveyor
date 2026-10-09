package cockpit

import (
	"fmt"
	"strconv"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
)

// LinkedFileInfo describes a file or a folder a prompt linked, and what the
// model saw of it, without its lines, which stay in the session
// (PromptFiles): lines From to To of Lines (a folder's entries), none where
// the file was binary, empty or could not be read, as Error says.
type LinkedFileInfo struct {
	Label     string `json:"label"`
	Path      string `json:"path"`
	Directory bool   `json:"directory,omitzero"`
	From      int    `json:"from,omitzero"`
	To        int    `json:"to,omitzero"`
	Lines     int    `json:"lines,omitzero"`
	Size      int64  `json:"size,omitzero"`
	Binary    bool   `json:"binary,omitzero"`
	Error     string `json:"error,omitempty"`
}

// LinkedFile is a file a prompt linked with what the model saw of it: its
// lines, as the file had them when the prompt was sent.
type LinkedFile struct {
	LinkedFileInfo
	Content string `json:"content,omitempty"`
}

// Describe says what the model saw of a file a prompt linked: "lines
// 1–100 of 345", "all 12 lines", "3 entries", "not read: …".
func (file LinkedFileInfo) Describe() string {
	lines := func(n int) string {
		if n == 1 {
			return "1 line"
		}
		return strconv.Itoa(n) + " lines"
	}
	switch {
	case file.Error != "":
		return "not read: " + file.Error
	case file.Directory && file.Lines == 0:
		return "empty folder"
	case file.Directory && file.To < file.Lines:
		return fmt.Sprintf("%d of %d entries", file.To, file.Lines)
	case file.Directory && file.Lines == 1:
		return "1 entry"
	case file.Directory:
		return fmt.Sprintf("%d entries", file.Lines)
	case file.Binary:
		return "binary · " + byteSize(int(file.Size))
	case file.Lines == 0 && file.Size == 0:
		return "empty"
	case file.From == 0 && file.Lines > 0:
		return lines(file.Lines) + " · none shown"
	case file.From == 0:
		return byteSize(int(file.Size)) + " · none shown"
	case file.From == 1 && file.To == file.Lines:
		return "all " + lines(file.Lines)
	}
	shown := fmt.Sprintf("lines %d–%d", file.From, file.To)
	if file.From == file.To {
		shown = "line " + strconv.Itoa(file.From)
	}
	if file.Lines > 0 {
		return fmt.Sprintf("%s of %d", shown, file.Lines)
	}
	return shown + " · " + byteSize(int(file.Size))
}

func linkedFileInfo(file contextbuilder.LinkedFile) LinkedFileInfo {
	// Session files are shown like any text.
	return LinkedFileInfo{
		Label: Headline(Clean(file.Label), 200), Path: Headline(Clean(file.Path), 400), Directory: file.Directory,
		From: file.From, To: file.To, Lines: file.Lines, Size: file.Size, Binary: file.Binary, Error: Headline(Clean(file.Error), 200),
	}
}

func linkedFileInfos(files []contextbuilder.LinkedFile) []LinkedFileInfo {
	if len(files) == 0 {
		return nil
	}
	infos := make([]LinkedFileInfo, len(files))
	for i, file := range files {
		infos[i] = linkedFileInfo(file)
	}
	return infos
}

// PromptFiles returns the files the prompt with the given message ID linked
// in a session, with the lines the model saw of each, as the session file
// keeps them. A prompt that linked none has none; a prompt the session does
// not hold is fs.ErrNotExist.
func PromptFiles(dir, id, messageID string) ([]LinkedFile, error) {
	payload, err := promptPayload(dir, id, messageID)
	if err != nil {
		return nil, err
	}
	files := decodePayload(payload).Files
	linked := make([]LinkedFile, len(files))
	for i, file := range files {
		linked[i] = LinkedFile{LinkedFileInfo: linkedFileInfo(file), Content: file.Content}
	}
	return linked, nil
}
