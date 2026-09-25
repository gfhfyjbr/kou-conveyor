package cockpit

import (
	"bufio"
	"bytes"
	"encoding/base64"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"unicode"
	"unicode/utf8"

	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

// Pictures the agent looked at. A ViewImage call reads an image for the
// model, which gets it as PNG or JPEG within its limits, and the session
// keeps those bytes with the call's operation. The transcript describes the
// picture of every call that read one (Tool.Image); a front-end that holds
// the transcript takes its bytes from there (Tool.Picture), one that does
// not asks the session file (ToolImage).

// viewedImage describes the picture of a ViewImage operation that read one,
// or is nil.
func viewedImage(state operation.ViewImageState) *ImageInfo {
	result := state.Result
	if result == nil || result.Error != "" || result.Content == "" || result.EncodedMIMEType == "" {
		return nil
	}
	content := result.Content
	info := &ImageInfo{
		Label:     viewedLabel(state.Path),
		MediaType: result.EncodedMIMEType,
		Size:      len(content)/4*3 - (len(content) - len(strings.TrimRight(content, "="))),
	}
	if config, _, err := decodeConfig(base64.NewDecoder(base64.StdEncoding, strings.NewReader(content))); err == nil {
		info.Width, info.Height = config.Width, config.Height
	}
	return info
}

// viewedLabel names a picture by the file it was read from.
func viewedLabel(path string) string { return Headline(Clean(filepath.Base(path)), 60) }

// viewedPicture is the picture of a ViewImage operation that read one.
func viewedPicture(state operation.ViewImageState) (Image, bool) {
	if viewedImage(state) == nil {
		return Image{}, false
	}
	data, err := base64.StdEncoding.DecodeString(state.Result.Content)
	if err != nil {
		return Image{}, false
	}
	return Image{Label: viewedLabel(state.Path), MediaType: state.Result.EncodedMIMEType, Data: data}, true
}

// Picture returns the picture a ViewImage call read, with its bytes, as the
// transcript holds it; a call that read none has none.
func (tool *Tool) Picture() (Image, bool) {
	for i := len(tool.operations) - 1; i >= 0; i-- {
		value := tool.operations[i]
		if value.Type != operation.TypeViewImage || value.Status != operation.StatusCompleted {
			continue
		}
		var state operation.ViewImageState
		if json.Unmarshal(value.State, &state) != nil {
			continue
		}
		if img, ok := viewedPicture(state); ok {
			return img, true
		}
	}
	return Image{}, false
}

// ValidCallID reports whether id can be a tool call's ID: providers make
// them, and front-ends name calls by them.
func ValidCallID(id string) bool {
	return id != "" && len(id) <= 256 && utf8.ValidString(id) && strings.IndexFunc(id, unicode.IsControl) < 0
}

// ToolImage returns the picture that tool call callID of a session read, as
// the session file keeps it. A call that read none — one still reading, one
// that failed, one of another tool, or no such call — is fs.ErrNotExist.
func ToolImage(dir, id, callID string) (Image, error) {
	if !ValidSessionID(id) {
		return Image{}, fmt.Errorf("invalid session ID %q", id)
	}
	path := SessionPath(dir, id)
	file, err := os.Open(path)
	if err != nil {
		return Image{}, err
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return Image{}, err
	}
	value, _ := viewedIndexes.LoadOrStore(path, &viewedIndex{})
	record, err := value.(*viewedIndex).find(file, info, callID)
	if err != nil {
		return Image{}, err
	}
	line := make([]byte, record.length)
	if _, err := file.ReadAt(line, record.offset); err != nil {
		return Image{}, fmt.Errorf("read the picture of call %s: %w", callID, err)
	}
	var decoded struct {
		Data struct {
			Operations []operation.Operation
			Operation  *operation.Operation
		} `json:"data"`
	}
	if err := json.Unmarshal(line, &decoded); err != nil {
		return Image{}, fmt.Errorf("read the picture of call %s: %w", callID, err)
	}
	operations := decoded.Data.Operations
	if decoded.Data.Operation != nil {
		operations = append(operations, *decoded.Data.Operation)
	}
	for _, value := range operations {
		if value.ID != record.operation {
			continue
		}
		state, err := operation.DecodeViewImageState(value)
		if err != nil {
			return Image{}, fmt.Errorf("read the picture of call %s: %w", callID, err)
		}
		if img, ok := viewedPicture(state); ok {
			return img, nil
		}
	}
	return Image{}, fmt.Errorf("call %s of session %s read no picture: %w", callID, id, fs.ErrNotExist)
}

// Where each session file keeps the pictures of its ViewImage calls: a
// session with many of them has its file read once, not once a picture.
// The file only grows while it is the same file; a rewound session's is
// another, read again from the start.
var viewedIndexes sync.Map // path → *viewedIndex

type viewedIndex struct {
	mu     sync.Mutex
	file   fs.FileInfo
	offset int64                   // how far the file was read
	calls  map[operation.ID]string // ViewImage operations → their calls
	found  map[string]viewedRecord // call → the record that holds its picture last
}

// viewedRecord is a line of a session file that holds a call's picture: the
// operation's completed state, in a status of the call or a checkpoint.
type viewedRecord struct {
	offset    int64
	length    int
	operation operation.ID
}

// viewImageType is in every record of a ViewImage operation; others are not
// decoded.
var viewImageType = []byte(operation.TypeViewImage)

// find returns the record that holds the picture of callID, reading what
// the file gained since it was last read.
func (x *viewedIndex) find(file *os.File, info fs.FileInfo, callID string) (viewedRecord, error) {
	x.mu.Lock()
	defer x.mu.Unlock()
	if x.file == nil || !os.SameFile(x.file, info) || info.Size() < x.offset {
		x.offset, x.calls, x.found = 0, make(map[operation.ID]string), make(map[string]viewedRecord)
	}
	x.file = info
	if record, ok := x.found[callID]; ok || info.Size() == x.offset {
		if !ok {
			return viewedRecord{}, fmt.Errorf("no picture of call %s: %w", callID, fs.ErrNotExist)
		}
		return record, nil
	}
	// Lines with pictures run to megabytes, past what a Scanner takes.
	reader := bufio.NewReaderSize(io.NewSectionReader(file, x.offset, info.Size()-x.offset), 256<<10)
	for {
		line, err := reader.ReadBytes('\n')
		if !bytes.HasSuffix(line, []byte{'\n'}) {
			// The end, or a record the runner is still writing: it is read
			// once it is whole.
			if err != nil && !errors.Is(err, io.EOF) {
				return viewedRecord{}, err
			}
			break
		}
		if bytes.Contains(line, viewImageType) {
			x.note(line, x.offset)
		}
		x.offset += int64(len(line))
	}
	record, ok := x.found[callID]
	if !ok {
		return viewedRecord{}, fmt.Errorf("no picture of call %s: %w", callID, fs.ErrNotExist)
	}
	return record, nil
}

// note takes in a record of a ViewImage operation: a status of its call,
// which names the call, or a checkpoint of the operation.
func (x *viewedIndex) note(line []byte, offset int64) {
	// The operations' states, pictures and all, are left undecoded.
	type head struct {
		ID     operation.ID
		Type   operation.Type
		Status operation.Status
	}
	var record struct {
		Type string `json:"type"`
		Data struct {
			Item struct {
				Kind string
				Data struct{ CallID string }
			}
			Operations []head
			Operation  *head
		} `json:"data"`
	}
	if json.Unmarshal(line, &record) != nil {
		return
	}
	var operations []head
	call := ""
	switch {
	case record.Type == "item" && record.Data.Item.Kind == "tool_call_status" && record.Data.Item.Data.CallID != "":
		call, operations = record.Data.Item.Data.CallID, record.Data.Operations
	case record.Type == "operation" && record.Data.Operation != nil:
		operations = []head{*record.Data.Operation}
	}
	for _, value := range operations {
		if value.Type != operation.TypeViewImage {
			continue
		}
		if call != "" {
			x.calls[value.ID] = call
		}
		owner, known := x.calls[value.ID]
		if known && value.Status == operation.StatusCompleted {
			x.found[owner] = viewedRecord{offset: offset, length: len(line), operation: value.ID}
		}
	}
}
