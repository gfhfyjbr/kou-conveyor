package operation

import (
	"bytes"
	"encoding/base64"
	"image"
	"image/color"
	"testing"

	"golang.org/x/image/tiff"
)

func FuzzPrepareViewImage(f *testing.F) {
	for _, format := range []string{"jpeg", "png", "bmp", "tiff", "webp", "gif"} {
		data := encodeViewImageFixture(f, viewImageFixture(8, 4), format)
		f.Add(data, uint16(0), uint16(65535), uint8(31), uint8(31))
		f.Add(data, uint16(7), uint16(1), uint8(1), uint8(2))
		f.Add(data[:len(data)/2], uint16(1), uint16(1023), uint8(15), uint8(15))
	}
	for _, source := range []image.Image{
		image.NewGray(image.Rect(0, 0, 8, 4)), image.NewGray16(image.Rect(0, 0, 8, 4)),
		image.NewRGBA64(image.Rect(0, 0, 8, 4)), image.NewNRGBA64(image.Rect(0, 0, 8, 4)),
		image.NewPaletted(image.Rect(0, 0, 8, 4), color.Palette{color.Black, color.White}),
	} {
		for _, compression := range []tiff.CompressionType{tiff.Uncompressed, tiff.Deflate} {
			var data bytes.Buffer
			if err := tiff.Encode(&data, source, &tiff.Options{Compression: compression}); err != nil {
				f.Fatal(err)
			}
			f.Add(data.Bytes(), uint16(11), uint16(65535), uint8(3), uint8(1))
		}
	}
	// A 3x2 lossy WebP generated with cwebp; the regular fixture is lossless.
	lossy, err := base64.StdEncoding.DecodeString("UklGRmgAAABXRUJQVlA4IFwAAACwAwCdASoDAAIAAUAmJagCdLoB+AH4gUoCqAP4BlAH6AAt2ZpmAAD+m7Pj/WW1boYWXX5+jgc+sAmPn/6jT4RcbKe7R78IuNlP/Tb/5jP1W6p5xhvvqt1T39SAAA==")
	if err != nil {
		f.Fatal(err)
	}
	f.Add(lossy, uint16(17), uint16(65535), uint8(31), uint8(31))
	f.Add([]byte(nil), uint16(0), uint16(1023), uint8(31), uint8(31))
	f.Fuzz(func(t *testing.T, data []byte, split, maxSize uint16, maxWidth, maxHeight uint8) {
		if len(data) > 64<<10 {
			t.Skip()
		}
		config := ViewImageConfig{
			MaxSize: int(maxSize) + 1, MaxWidth: int(maxWidth) + 1, MaxHeight: int(maxHeight) + 1,
			MaxSourcePixels: 4096,
		}
		offset := int(split) % (len(data) + 1)
		result, err := prepareViewImage(t.Context(), [][]byte{data[:offset], data[offset:]}, config, nil)
		if err != nil {
			if result.Content != "" {
				t.Fatal("failed preparation returned encoded content")
			}
			return
		}
		if len(result.Content) > config.MaxSize || result.ScaleRatio <= 0 || result.ScaleRatio > 1 ||
			result.OriginalWidth <= 0 || result.OriginalHeight <= 0 || result.Error != "" {
			t.Fatalf("invalid successful result: %+v", result)
		}
		encoded, err := base64.StdEncoding.DecodeString(result.Content)
		if err != nil {
			t.Fatal(err)
		}
		decoded, format, err := image.Decode(bytes.NewReader(encoded))
		if err != nil {
			t.Fatalf("invalid output image: %v", err)
		}
		wantFormat := "png"
		if result.OriginalMIMEType == "image/jpeg" {
			wantFormat = "jpeg"
		}
		if format != wantFormat || result.EncodedMIMEType != "image/"+format ||
			decoded.Bounds().Dx() > config.MaxWidth || decoded.Bounds().Dy() > config.MaxHeight {
			t.Fatalf("output violates format or dimension limits: format=%s bounds=%v config=%+v", format, decoded.Bounds(), config)
		}
	})
}
