package contextbuilder

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// External input carries a message from the user. Its payload is a JSON
// string of the message's text, as it has always been, or, when images or
// linked files come with the text, an object: {"Text": string, "Images":
// [{"Label": string, "URL": string}], "Files": [LinkedFile]}. The images
// reach the model beside the text, each after its label, which is how the
// text refers to it; the linked files follow the text, each under its label.

// Message is what a user's message brings: its text, and the images and
// linked files that come with it.
type Message struct {
	Text   string
	Images []llm.Image  `json:",omitzero"`
	Files  []LinkedFile `json:",omitzero"`
}

// LinkedFile is a file or a folder a user's message links to by a reference
// in its text, such as "$cmd/main.go" or "$cmd/main.go:10-40": the part of
// it the model sees, as it was when the message was sent, and what the
// model needs to read the rest itself. A message can link a file and show
// none of it: an empty file, a binary one, or one that could not be read,
// as Error says.
type LinkedFile struct {
	// Label is how the message's text refers to the file.
	Label string `json:",omitzero"`
	// Path is where the model finds the file: relative to the workspace,
	// or absolute.
	Path string
	// Directory marks a folder: Content lists its entries, one a line,
	// folders with a slash at the end.
	Directory bool `json:",omitzero"`
	// From and To are the lines of the file Content holds, counting from 1
	// (a folder's entries); both are 0 when it holds none.
	From int `json:",omitzero"`
	To   int `json:",omitzero"`
	// Lines is how many lines the file has (entries, for a folder), or 0
	// where they were not counted: a file too large to count.
	Lines int `json:",omitzero"`
	// Size is the size of the file in bytes.
	Size int64 `json:",omitzero"`
	// Content is lines From to To, without their line numbers. A line too
	// long to show whole is cut short, and says so.
	Content string `json:",omitzero"`
	// Binary marks a file that is not text, which shows nothing.
	Binary bool `json:",omitzero"`
	// Error says why the file shows nothing: it did not exist when the
	// message was sent, or could not be read.
	Error string `json:",omitzero"`
}

// ExternalPayload encodes a user's message as the payload of external input:
// text alone stays a JSON string, so earlier readers of sessions read it.
func ExternalPayload(text string, images []llm.Image) (jsontext.Value, error) {
	return EncodeMessage(Message{Text: text, Images: images})
}

// ExternalMessage decodes the payload of external input in either form: the
// message's text and the images that came with it.
func ExternalMessage(payload jsontext.Value) (string, []llm.Image, error) {
	message, err := DecodeMessage(payload)
	return message.Text, message.Images, err
}

// EncodeMessage encodes a user's message as the payload of external input:
// text alone stays a JSON string, so earlier readers of sessions read it.
func EncodeMessage(message Message) (jsontext.Value, error) {
	if len(message.Images) == 0 && len(message.Files) == 0 {
		return json.Marshal(message.Text)
	}
	if err := validateImages(message.Images); err != nil {
		return nil, err
	}
	if err := validateFiles(message.Files); err != nil {
		return nil, err
	}
	return json.Marshal(message)
}

// DecodeMessage decodes the payload of external input in either form.
func DecodeMessage(payload jsontext.Value) (Message, error) {
	switch payload.Kind() {
	case jsontext.KindString:
		var text string
		err := json.Unmarshal(payload, &text)
		return Message{Text: text}, err
	case jsontext.KindBeginObject:
		var message Message
		if err := json.Unmarshal(payload, &message, json.RejectUnknownMembers(true)); err != nil {
			return Message{}, err
		}
		// The object form carries images or files; text alone is a JSON
		// string.
		if len(message.Images) == 0 && len(message.Files) == 0 {
			return Message{}, errors.New("a message without images or files is a JSON string of its text")
		}
		if err := validateImages(message.Images); err != nil {
			return Message{}, err
		}
		if err := validateFiles(message.Files); err != nil {
			return Message{}, err
		}
		return message, nil
	}
	return Message{}, errors.New("the payload is neither text nor a message with images or files")
}

func validateImages(images []llm.Image) error {
	for index, image := range images {
		if strings.TrimSpace(image.URL) == "" {
			return fmt.Errorf("image %d has no URL", index+1)
		}
	}
	return nil
}

func validateFiles(files []LinkedFile) error {
	for index, file := range files {
		switch {
		case strings.TrimSpace(file.Path) == "":
			return fmt.Errorf("linked file %d has no path", index+1)
		case file.From < 0 || file.To < file.From || (file.From == 0) != (file.To == 0):
			return fmt.Errorf("linked file %d shows lines %d to %d", index+1, file.From, file.To)
		case file.Lines < 0 || file.Size < 0:
			return fmt.Errorf("linked file %d has a negative size", index+1)
		}
	}
	return nil
}
