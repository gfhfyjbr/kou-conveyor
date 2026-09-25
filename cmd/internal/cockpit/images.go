package cockpit

import (
	"bufio"
	"bytes"
	"encoding/base64"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"image"
	"image/color"
	_ "image/gif"
	"image/jpeg"
	"image/png"
	"io"
	"os"
	"regexp"
	"slices"
	"strconv"
	"strings"

	"golang.org/x/image/draw"

	_ "golang.org/x/image/bmp"
	_ "golang.org/x/image/tiff"
	_ "golang.org/x/image/webp"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// Images. A prompt may bring images: pasted into a composer, each is referred
// to in the prompt's text by its label, "[Image 1]" for the first, and goes
// to the model beside the text, right after its label. An image whose label
// the text no longer holds is left out, so taking a label out of the prompt
// takes its image out too.

// Image is a picture that goes with a prompt: its bytes, their media type and
// the label the prompt's text refers to it by.
type Image struct {
	Label     string `json:"label"`
	MediaType string `json:"media_type"`
	Data      []byte `json:"data"`
}

// ImageInfo describes an image without its bytes, for those who show it.
type ImageInfo struct {
	Label     string `json:"label"`
	MediaType string `json:"media_type,omitempty"`
	Width     int    `json:"width,omitempty"`
	Height    int    `json:"height,omitempty"`
	Size      int    `json:"size,omitempty"` // bytes
}

const (
	// MaxImageSide bounds the sides of an image that goes to a model: a
	// larger one is scaled down, as providers would do themselves.
	MaxImageSide = 2000
	// maxImageBytes bounds the bytes of an image that goes to a model: their
	// base64 must fit the 5 MB the Messages API takes.
	maxImageBytes = 3_700_000
	// MaxImageSource bounds what can be pasted, before it is scaled down.
	MaxImageSource = 64 << 20
	// maxSourcePixels bounds the pixels decoded from what is pasted.
	maxSourcePixels = 64_000_000
	// MaxImages bounds the images of one prompt; the runner takes no more.
	MaxImages = 20
)

// ImageLabel is how a prompt's text refers to its image number n.
func ImageLabel(n int) string { return "[Image " + strconv.Itoa(n) + "]" }

var labelPattern = regexp.MustCompile(`\[Image ([1-9][0-9]{0,5})\]`)

// imageNumber is the number of a label such as "[Image 3]", or 0.
func imageNumber(label string) int {
	match := labelPattern.FindStringSubmatch(label)
	if match == nil || match[0] != label {
		return 0
	}
	n, _ := strconv.Atoi(match[1])
	return n
}

// NextImageNumber is the number of the next image pasted into text, beside
// the images it has: one past any label either uses.
func NextImageNumber(text string, images []Image) int {
	next := 1
	for _, image := range images {
		next = max(next, imageNumber(image.Label)+1)
	}
	for _, match := range labelPattern.FindAllStringSubmatch(text, -1) {
		n, _ := strconv.Atoi(match[1])
		next = max(next, n+1)
	}
	return next
}

// Referenced returns the images whose labels text holds, each once, in the
// order of the images, and at most MaxImages of them.
func Referenced(text string, images []Image) []Image {
	var kept []Image
	for _, image := range images {
		if image.Label == "" || !strings.Contains(text, image.Label) ||
			slices.ContainsFunc(kept, func(other Image) bool { return other.Label == image.Label }) {
			continue
		}
		kept = append(kept, image)
		if len(kept) == MaxImages {
			break
		}
	}
	return kept
}

// ImageInfos describes images.
func ImageInfos(images []Image) []ImageInfo {
	if len(images) == 0 {
		return nil
	}
	infos := make([]ImageInfo, len(images))
	for i, image := range images {
		infos[i] = image.Info()
	}
	return infos
}

// Info describes the image: its size in pixels when its header says.
func (img Image) Info() ImageInfo {
	info := ImageInfo{Label: img.Label, MediaType: img.MediaType, Size: len(img.Data)}
	if config, _, err := image.DecodeConfig(bytes.NewReader(img.Data)); err == nil {
		info.Width, info.Height = config.Width, config.Height
	}
	return info
}

// mediaTypes are the formats a model takes as they are, by the name the
// image package gives them.
var mediaTypes = map[string]string{"png": "image/png", "jpeg": "image/jpeg", "gif": "image/gif", "webp": "image/webp"}

// PrepareImage makes pasted bytes an image a model takes: PNG, JPEG, GIF or
// WebP within MaxImageSide pixels a side and the bytes a provider takes. An
// image that already is one stays as it is; others are converted, and scaled
// down as far as they must. BMP and TIFF are read too.
func PrepareImage(data []byte) (Image, error) {
	switch {
	case len(data) == 0:
		return Image{}, errors.New("the image is empty")
	case len(data) > MaxImageSource:
		return Image{}, fmt.Errorf("the image is %s; images of up to %s are taken", byteSize(len(data)), byteSize(MaxImageSource))
	}
	config, format, err := image.DecodeConfig(bytes.NewReader(data))
	switch {
	case err != nil:
		return Image{}, errors.New("not an image the cockpit reads: PNG, JPEG, GIF, WebP, BMP and TIFF are")
	case config.Width <= 0 || config.Height <= 0:
		return Image{}, errors.New("the image has no pixels")
	case config.Width > maxSourcePixels/config.Height:
		return Image{}, fmt.Errorf("the image is %d×%d pixels, too large to read", config.Width, config.Height)
	}
	if mediaType, ok := mediaTypes[format]; ok && config.Width <= MaxImageSide && config.Height <= MaxImageSide && len(data) <= maxImageBytes {
		return Image{MediaType: mediaType, Data: data}, nil
	}
	source, _, err := image.Decode(bytes.NewReader(data))
	if err != nil {
		return Image{}, fmt.Errorf("read the %s image: %w", format, err)
	}
	scale := min(1, float64(MaxImageSide)/float64(config.Width), float64(MaxImageSide)/float64(config.Height))
	for range 6 {
		scaled := Scale(source, max(1, int(float64(config.Width)*scale+0.5)), max(1, int(float64(config.Height)*scale+0.5)))
		// Photos keep their JPEG; the rest, screenshots mostly, go as PNG
		// while it is small enough.
		if format != "jpeg" {
			var encoded bytes.Buffer
			if png.Encode(&encoded, scaled) == nil && encoded.Len() <= maxImageBytes {
				return Image{MediaType: "image/png", Data: encoded.Bytes()}, nil
			}
		}
		opaque := flatten(scaled)
		for _, quality := range []int{90, 80, 70} {
			var encoded bytes.Buffer
			if jpeg.Encode(&encoded, opaque, &jpeg.Options{Quality: quality}) == nil && encoded.Len() <= maxImageBytes {
				return Image{MediaType: "image/jpeg", Data: encoded.Bytes()}, nil
			}
		}
		scale *= 0.7
	}
	return Image{}, errors.New("the image does not compress to a size a model takes")
}

// Scale draws src at width×height pixels.
func Scale(src image.Image, width, height int) image.Image {
	bounds := src.Bounds()
	if bounds.Dx() == width && bounds.Dy() == height {
		return src
	}
	scaled := image.NewNRGBA(image.Rect(0, 0, width, height))
	draw.CatmullRom.Scale(scaled, scaled.Bounds(), src, bounds, draw.Src, nil)
	return scaled
}

// flatten lays an image with transparency over white, as JPEG has none.
func flatten(src image.Image) image.Image {
	opaque := image.NewRGBA(src.Bounds())
	draw.Draw(opaque, opaque.Bounds(), image.NewUniform(color.White), image.Point{}, draw.Src)
	draw.Draw(opaque, opaque.Bounds(), src, src.Bounds().Min, draw.Over)
	return opaque
}

func byteSize(n int) string {
	switch {
	case n < 1<<10:
		return fmt.Sprintf("%d B", n)
	case n < 1<<20:
		return fmt.Sprintf("%d KB", n>>10)
	}
	return fmt.Sprintf("%.1f MB", float64(n)/(1<<20))
}

// ---------------------------------------------------------------- sessions

// payloadMessage reads the payload of a prompt: its text and its images.
func payloadMessage(payload jsontext.Value) (string, []llm.Image) {
	text, images, err := contextbuilder.ExternalMessage(payload)
	if err != nil {
		return string(payload), nil
	}
	return text, images
}

// imageInfo describes an image a prompt recorded, by its reference: a data
// URL tells its media type, size and pixels.
func imageInfo(image llm.Image) ImageInfo {
	// Labels come from session files, like any text shown.
	info := ImageInfo{Label: Headline(Clean(image.Label), 40)}
	rest, ok := strings.CutPrefix(image.URL, "data:")
	if !ok {
		return info
	}
	header, data, _ := strings.Cut(rest, ",")
	info.MediaType, _, _ = strings.Cut(header, ";")
	info.Size = len(data)/4*3 - (len(data) - len(strings.TrimRight(data, "=")))
	if config, _, err := decodeConfig(base64.NewDecoder(base64.StdEncoding, strings.NewReader(data))); err == nil {
		info.Width, info.Height = config.Width, config.Height
	}
	return info
}

var decodeConfig = image.DecodeConfig

// imageOf is the image a prompt recorded, with its bytes: only data URLs
// have them.
func imageOf(recorded llm.Image) (Image, bool) {
	rest, ok := strings.CutPrefix(recorded.URL, "data:")
	if !ok {
		return Image{}, false
	}
	header, data, _ := strings.Cut(rest, ",")
	mediaType, encoding, _ := strings.Cut(header, ";")
	if encoding != "base64" {
		return Image{}, false
	}
	decoded, err := base64.StdEncoding.DecodeString(data)
	if err != nil {
		return Image{}, false
	}
	return Image{Label: recorded.Label, MediaType: mediaType, Data: decoded}, true
}

// promptImages returns the images of a recorded prompt that have bytes.
func promptImages(payload jsontext.Value) []Image {
	_, recorded := payloadMessage(payload)
	var images []Image
	for _, image := range recorded {
		if img, ok := imageOf(image); ok {
			images = append(images, img)
		}
	}
	return images
}

// PromptImages returns the images the prompt with the given message ID
// brought to a session, as the session file keeps them. A prompt without
// images has none; a prompt the session does not hold is fs.ErrNotExist.
func PromptImages(dir, id, messageID string) ([]Image, error) {
	if !ValidSessionID(id) {
		return nil, fmt.Errorf("invalid session ID %q", id)
	}
	file, err := os.Open(SessionPath(dir, id))
	if err != nil {
		return nil, err
	}
	defer file.Close()
	// Lines with images run to megabytes, past what a Scanner takes.
	reader := bufio.NewReaderSize(file, 256<<10)
	needle := []byte(messageID)
	for {
		line, err := reader.ReadBytes('\n')
		if bytes.Contains(line, needle) && bytes.HasSuffix(line, []byte{'\n'}) {
			var record struct {
				Type string `json:"type"`
				Data struct {
					Item struct {
						Kind string
						Data inbox.Input
					}
				} `json:"data"`
			}
			input := &record.Data.Item.Data
			if json.Unmarshal(line, &record) == nil && record.Type == "item" && record.Data.Item.Kind == "input" &&
				input.Kind == inbox.InputExternal && string(input.ID) == messageID {
				return promptImages(input.Payload), nil
			}
		}
		if errors.Is(err, io.EOF) {
			return nil, fmt.Errorf("prompt %s is not in session %s: %w", messageID, id, os.ErrNotExist)
		}
		if err != nil {
			return nil, err
		}
	}
}

// ---------------------------------------------------------------- queues

// AddImages queues a prompt that brings images, as Add does, and returns it.
func (q *Queue) AddImages(text, model string, images []Image) QueueItem {
	item := q.Add(text, model)
	q.SetImages(item.ID, images)
	item, _, _ = q.Get(item.ID)
	return item
}

// SetImages gives a queued prompt the images its text refers to.
func (q *Queue) SetImages(id string, images []Image) bool {
	i := q.index(id)
	if i < 0 {
		return false
	}
	q.Items[i].Images = Referenced(q.Items[i].Text, images)
	return true
}

// MarshalJSON describes the prompt's images without their bytes, which stay
// with the queue: front-ends get them by the prompt's ID when they need them.
func (item QueueItem) MarshalJSON() ([]byte, error) {
	type plain QueueItem
	return json.Marshal(struct {
		plain
		Images []ImageInfo `json:"images,omitempty"`
	}{plain(item), ImageInfos(item.Images)})
}

// payloadEntry reads a prompt's payload for the transcript: its text, and
// what its images are.
func payloadEntry(payload jsontext.Value) (string, []ImageInfo) {
	text, recorded := payloadMessage(payload)
	var infos []ImageInfo
	for _, image := range recorded {
		infos = append(infos, imageInfo(image))
	}
	return Clean(text), infos
}

// WithImages describes the images a prompt submitted with Submit brings.
func (t *Transcript) WithImages(e *Entry, images []Image) *Entry {
	e.Images = ImageInfos(images)
	return t.touch(e)
}
