package responsesapi

import (
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// Images a model cannot read. Some models behind a Responses API gateway
// take text only, and reject the whole request when it carries an image —
// "messages.content.type is invalid, allowed values: ['text']" — so a picture
// a ViewImage call left in the conversation would fail every request of the
// run after it, compactions included. When a request with images is rejected
// so, the adapter warns, sends it again without them, and leaves the images
// out of the run's later requests. A message keeps its text and a tool result
// a note in each image's place, so the model knows what it was not shown.

// imageLeftOut stands in for a tool result's image the model cannot read.
const imageLeftOut = "[An image was left out here: this model cannot read images.]"

// imagesRejected reports a rejection of the images a request carried: the
// gateway's content type error, or one that names images as not supported
// or invalid. It is only acted on for a request that had images, which is
// sent again once at most, so a broad match costs no more than a retry.
func imagesRejected(err *APIError) bool {
	if err == nil {
		return false
	}
	message := strings.ToLower(err.Message)
	if strings.Contains(message, "content.type is invalid") {
		return true
	}
	if !strings.Contains(message, "image") {
		return false
	}
	for _, phrase := range []string{"not support", "unsupported", "only supported", "invalid", "not allowed", "not a valid"} {
		if strings.Contains(message, phrase) {
			return true
		}
	}
	return false
}

// hasImages reports whether a message or a tool result of the request
// carries an image.
func hasImages(request llm.Request) bool {
	for _, item := range request.Input {
		if _, changed := itemWithoutImages(item); changed {
			return true
		}
	}
	return false
}

// withoutImages is the request with its images left out. The input and the
// items that change are copies: the caller's request stays as it was.
func withoutImages(request llm.Request) llm.Request {
	var input []llm.Item
	for index, item := range request.Input {
		stripped, changed := itemWithoutImages(item)
		switch {
		case changed && input == nil:
			input = append(make([]llm.Item, 0, len(request.Input)), request.Input[:index]...)
			input = append(input, stripped)
		case input != nil:
			input = append(input, stripped)
		}
	}
	if input != nil {
		request.Input = input
	}
	return request
}

// itemWithoutImages is the item with its images left out, and whether it had
// any.
func itemWithoutImages(item llm.Item) (llm.Item, bool) {
	switch data := item.Data.(type) {
	case llm.Message:
		if len(data.Images) == 0 {
			return item, false
		}
		labels := make([]string, 0, len(data.Images))
		for _, image := range data.Images {
			if label := strings.TrimSpace(image.Label); label != "" {
				labels = append(labels, label)
			}
		}
		note := "[The images of this message were left out: this model cannot read images.]"
		if len(labels) != 0 {
			note = "[The images of this message were left out: this model cannot read images. They were " +
				strings.Join(labels, ", ") + ".]"
		}
		data.Images = nil
		data.Text = strings.TrimSpace(data.Text + "\n\n" + note)
		item.Data = data
		return item, true
	case llm.ToolResult:
		var output []llm.ToolResultOutput
		for index, part := range data.Output {
			if part.Kind != llm.ToolResultImage {
				if output != nil {
					output = append(output, part)
				}
				continue
			}
			if output == nil {
				output = append(make([]llm.ToolResultOutput, 0, len(data.Output)), data.Output[:index]...)
			}
			output = append(output, llm.ToolResultOutput{Kind: llm.ToolResultText, Value: imageLeftOut})
		}
		if output == nil {
			return item, false
		}
		data.Output = output
		item.Data = data
		return item, true
	}
	return item, false
}
