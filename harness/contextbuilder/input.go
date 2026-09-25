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
// string of the message's text, as it has always been, or, when images come
// with the text, an object: {"Text": string, "Images": [{"Label": string,
// "URL": string}]}. The images reach the model beside the text, each after
// its label, which is how the text refers to it.
type externalMessage struct {
	Text   string
	Images []llm.Image `json:",omitzero"`
}

// ExternalPayload encodes a user's message as the payload of external input:
// text alone stays a JSON string, so earlier readers of sessions read it.
func ExternalPayload(text string, images []llm.Image) (jsontext.Value, error) {
	if len(images) == 0 {
		return json.Marshal(text)
	}
	if err := validateImages(images); err != nil {
		return nil, err
	}
	return json.Marshal(externalMessage{Text: text, Images: images})
}

// ExternalMessage decodes the payload of external input in either form: the
// message's text and the images that came with it.
func ExternalMessage(payload jsontext.Value) (string, []llm.Image, error) {
	switch payload.Kind() {
	case jsontext.KindString:
		var text string
		err := json.Unmarshal(payload, &text)
		return text, nil, err
	case jsontext.KindBeginObject:
		var message externalMessage
		if err := json.Unmarshal(payload, &message, json.RejectUnknownMembers(true)); err != nil {
			return "", nil, err
		}
		// The object form carries images; text alone is a JSON string.
		if len(message.Images) == 0 {
			return "", nil, errors.New("a message without images is a JSON string of its text")
		}
		if err := validateImages(message.Images); err != nil {
			return "", nil, err
		}
		return message.Text, message.Images, nil
	}
	return "", nil, errors.New("the payload is neither text nor a message with images")
}

func validateImages(images []llm.Image) error {
	for index, image := range images {
		if strings.TrimSpace(image.URL) == "" {
			return fmt.Errorf("image %d has no URL", index+1)
		}
	}
	return nil
}
