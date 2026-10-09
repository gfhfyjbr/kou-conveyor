package contextbuilder

import (
	"bytes"
	"encoding/base64"
	"image"
	_ "image/gif"
	_ "image/jpeg"
	_ "image/png"
	"strings"

	_ "golang.org/x/image/webp"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// Images. A provider bills an image by its pixels, about a token for every
// 750 once it has scaled the image down to its largest size, and refuses a
// request with too many images or too many bytes: a compaction request of a
// long session once carried 74 screenshots in 34.6 MB, past Anthropic's
// 32 MB. An image is estimated from the size its header gives, and the
// conversation keeps maxImages images and maxImageBytes of them at most:
// past either, the oldest are left out, in a batch, as pruning cuts tool
// results.

const (
	// pixelsPerToken and maxImageEdge are how the providers count an image:
	// Claude scales an image to fit its largest edge, then bills a token
	// for every 750 pixels; OpenAI bills less.
	pixelsPerToken = 750
	maxImageEdge   = 2576
	// imageHeaderBytes is how much of a data URL is read for the header,
	// which leads the image; a JPEG's may follow its metadata.
	imageHeaderBytes = 96 << 10

	// maxImages and maxImageBytes bound the images of the conversation, in
	// number and in the bytes of their data URLs; what is left out leaves a
	// quarter of each bound free.
	maxImages     = 40
	maxImageBytes = 16 << 20
)

// leftOutImage stands in for an image left out of the conversation.
const leftOutImage = "[An image was left out of the context here to keep the requests within the provider's limits; view it again if it is still needed.]"

// imageTokensOf estimates the tokens of an image from the size a data URL's
// header gives; an image whose size it cannot tell counts imageTokens.
func imageTokensOf(url string) int64 {
	data, ok := strings.CutPrefix(url, "data:")
	if !ok {
		return imageTokens
	}
	if _, data, ok = strings.Cut(data, ";base64,"); !ok {
		return imageTokens
	}
	prefix := data[:min(len(data), imageHeaderBytes)]
	decoded, err := base64.StdEncoding.DecodeString(prefix[:len(prefix)/4*4])
	if err != nil {
		return imageTokens
	}
	config, _, err := image.DecodeConfig(bytes.NewReader(decoded))
	if err != nil || config.Width <= 0 || config.Height <= 0 {
		return imageTokens
	}
	width, height := float64(config.Width), float64(config.Height)
	if scale := maxImageEdge / max(width, height); scale < 1 {
		width, height = width*scale, height*scale
	}
	return max(int64(width*height/pixelsPerToken), 1)
}

// boundImages leaves the oldest images of the committed conversation out
// once it holds more than maxImages or maxImageBytes of them, until a
// quarter of each bound is free, and reports how many it left out.
func (current *builder) boundImages() int {
	count, size := 0, 0
	for _, item := range current.committedPrefix {
		for _, url := range imagesOf(item) {
			count++
			size += len(url)
		}
	}
	if count <= maxImages && size <= maxImageBytes {
		return 0
	}
	left := 0
	for index, item := range current.committedPrefix {
		if count <= maxImages*3/4 && size <= maxImageBytes*3/4 {
			break
		}
		urls := imagesOf(item)
		if len(urls) == 0 {
			continue
		}
		current.committedPrefix[index] = withoutImages(item)
		for _, url := range urls {
			count--
			size -= len(url)
		}
		left += len(urls)
	}
	// The provider's count of the request no longer holds.
	current.usage, current.usageMark = 0, 0
	current.imagesLeftOut += left
	return left
}

// imagesOf returns the images of an item: a message's, or a tool result's.
func imagesOf(item llm.Item) []string {
	var urls []string
	switch data := item.Data.(type) {
	case llm.Message:
		for _, image := range data.Images {
			urls = append(urls, image.URL)
		}
	case llm.ToolResult:
		for _, part := range data.Output {
			if part.Kind == llm.ToolResultImage {
				urls = append(urls, part.Value)
			}
		}
	}
	return urls
}

// withoutImages is an item with its images left out: a message says which
// of its images went, and a tool result has a note in each one's place.
func withoutImages(item llm.Item) llm.Item {
	switch data := item.Data.(type) {
	case llm.Message:
		note := "[The images of this message were left out of the context to keep the requests within the provider's limits"
		var labels []string
		for _, image := range data.Images {
			if image.Label != "" {
				labels = append(labels, image.Label)
			}
		}
		if len(labels) != 0 {
			note += ": " + strings.Join(labels, ", ")
		}
		data.Images = nil
		data.Text = strings.TrimSpace(data.Text + "\n\n" + note + ".]")
		item.Data = data
	case llm.ToolResult:
		output := make([]llm.ToolResultOutput, len(data.Output))
		for index, part := range data.Output {
			output[index] = part
			if part.Kind == llm.ToolResultImage {
				output[index] = llm.ToolResultOutput{Kind: llm.ToolResultText, Value: leftOutImage}
			}
		}
		data.Output = output
		item.Data = data
	}
	return item
}
