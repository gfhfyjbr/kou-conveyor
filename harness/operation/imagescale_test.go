package operation

import (
	"bytes"
	"context"
	"encoding/base64"
	"fmt"
	"image"
	"image/color"
	"image/jpeg"
	"math/rand/v2"
	"runtime"
	"testing"

	"golang.org/x/image/draw"
)

// anyImage hides the type of the image it holds, as an image of a type
// draw has no path of its own for.
type anyImage struct{ image.Image }

// scaleSources are images of each kind a decoder gives and draw's scaler
// takes a path of its own for, filled at random.
func scaleSources(random *rand.Rand, r image.Rectangle) map[string]image.Image {
	fill := func(pix []uint8) {
		for i := range pix {
			pix[i] = uint8(random.UintN(256))
		}
	}
	sources := map[string]image.Image{}

	gray := image.NewGray(r)
	fill(gray.Pix)
	sources["gray"] = gray

	nrgba := image.NewNRGBA(r)
	fill(nrgba.Pix)
	sources["nrgba"] = nrgba
	opaque := image.NewNRGBA(r)
	fill(opaque.Pix)
	for i := 3; i < len(opaque.Pix); i += 4 {
		opaque.Pix[i] = 0xff
	}
	sources["opaque nrgba"] = opaque
	sources["any"] = anyImage{nrgba}

	rgba := image.NewRGBA(r)
	for i := 0; i < len(rgba.Pix); i += 4 {
		a := uint8(random.UintN(256))
		rgba.Pix[i+3] = a
		for c := range 3 {
			rgba.Pix[i+c] = uint8(random.UintN(uint(a) + 1))
		}
	}
	sources["rgba"] = rgba
	// image.RGBA holds any bytes, colour over its alpha too, which draw clamps.
	loose := image.NewRGBA(r)
	fill(loose.Pix)
	sources["rgba over its alpha"] = loose

	for _, ratio := range []image.YCbCrSubsampleRatio{
		image.YCbCrSubsampleRatio444, image.YCbCrSubsampleRatio422, image.YCbCrSubsampleRatio420,
		image.YCbCrSubsampleRatio440, image.YCbCrSubsampleRatio411, image.YCbCrSubsampleRatio410,
	} {
		ycbcr := image.NewYCbCr(r, ratio)
		fill(ycbcr.Y)
		fill(ycbcr.Cb)
		fill(ycbcr.Cr)
		sources["ycbcr "+ratio.String()] = ycbcr
	}
	nycbcra := image.NewNYCbCrA(r, image.YCbCrSubsampleRatio420)
	fill(nycbcra.Y)
	fill(nycbcra.Cb)
	fill(nycbcra.Cr)
	fill(nycbcra.A)
	sources["nycbcra"] = nycbcra

	palette := make(color.Palette, 16)
	for i := range palette {
		palette[i] = color.NRGBA{uint8(random.UintN(256)), uint8(random.UintN(256)), uint8(random.UintN(256)), uint8(random.UintN(256))}
	}
	paletted := image.NewPaletted(r, palette)
	for i := range paletted.Pix {
		paletted.Pix[i] = uint8(random.UintN(16))
	}
	sources["paletted"] = paletted

	cmyk := image.NewCMYK(r)
	fill(cmyk.Pix)
	sources["cmyk"] = cmyk
	gray16 := image.NewGray16(r)
	fill(gray16.Pix)
	sources["gray16"] = gray16
	nrgba64 := image.NewNRGBA64(r)
	fill(nrgba64.Pix)
	sources["nrgba64"] = nrgba64
	return sources
}

func TestScaleBiLinearDrawsWhatDrawDoes(t *testing.T) {
	random := rand.New(rand.NewPCG(3, 4))
	for _, size := range []struct{ source, target image.Rectangle }{
		{image.Rect(0, 0, 1, 1), image.Rect(0, 0, 1, 1)},
		{image.Rect(0, 0, 7, 5), image.Rect(0, 0, 3, 2)},
		{image.Rect(0, 0, 300, 200), image.Rect(0, 0, 200, 133)},
		{image.Rect(-3, 5, 197, 305), image.Rect(0, 0, 37, 301)},
		{image.Rect(0, 0, 97, 61), image.Rect(0, 0, 96, 60)},
		{image.Rect(0, 0, 2, 900), image.Rect(0, 0, 1, 300)},
		{image.Rect(0, 0, 900, 3), image.Rect(0, 0, 41, 1)},
		{image.Rect(0, 0, 64, 48), image.Rect(0, 0, 128, 96)},
		{image.Rect(11, -7, 61, 33), image.Rect(4, 9, 33, 30)},
		{image.Rect(0, 0, 1000, 700), image.Rect(0, 0, 391, 274)},
	} {
		for name, source := range scaleSources(random, size.source) {
			t.Run(fmt.Sprintf("%s %v to %v", name, size.source.Size(), size.target), func(t *testing.T) {
				want := image.NewNRGBA(size.target)
				draw.BiLinear.Scale(want, want.Bounds(), source, source.Bounds(), draw.Src, nil)
				got := image.NewNRGBA(size.target)
				scaleBiLinear(got, source)
				if !bytes.Equal(got.Pix, want.Pix) {
					for i := range got.Pix {
						if got.Pix[i] != want.Pix[i] {
							pixel := i / 4
							t.Fatalf("pixel (%d, %d) is %v, draw draws %v",
								pixel%got.Rect.Dx(), pixel/got.Rect.Dx(), got.Pix[pixel*4:pixel*4+4], want.Pix[pixel*4:pixel*4+4])
						}
					}
				}
			})
		}
	}
}

func TestScaleBiLinearKeepsOnlyTheRowsItDraws(t *testing.T) {
	source := image.NewNRGBA(image.Rect(0, 0, 400, 8000))
	for i := range source.Pix {
		source.Pix[i] = uint8(i)
	}
	target := image.NewNRGBA(image.Rect(0, 0, 100, 2000))
	var before, after runtime.MemStats
	runtime.ReadMemStats(&before)
	scaleBiLinear(target, source)
	runtime.ReadMemStats(&after)
	// draw keeps every row of the source scaled to the width of the target:
	// 100 × 8000 × 32 bytes, 25.6 MB.
	if allocated := after.TotalAlloc - before.TotalAlloc; allocated > 1<<20 {
		t.Errorf("scaling 400×8000 to 100×2000 allocated %d bytes, want at most 1 MiB", allocated)
	}
}

func TestScaleBiLinearTakesDecodedImagesWithoutACallPerPixel(t *testing.T) {
	random := rand.New(rand.NewPCG(7, 8))
	for name, source := range scaleSources(random, image.Rect(0, 0, 300, 200)) {
		switch name {
		case "any", "ycbcr " + image.YCbCrSubsampleRatio411.String(), "ycbcr " + image.YCbCrSubsampleRatio410.String():
			continue // draw reads these with At, an allocation a pixel, and so does scaleBiLinear
		}
		target := image.NewNRGBA(image.Rect(0, 0, 200, 133))
		allocations := testing.AllocsPerRun(2, func() { scaleBiLinear(target, source) })
		if allocations > 20 {
			t.Errorf("scaling a 300×200 %s made %.0f allocations, want at most 20", name, allocations)
		}
	}
}

// countingImage counts the pixels read from it, with At as an image of a
// type draw has no path of its own for.
type countingImage struct {
	image.Image
	reads *int
}

func (m countingImage) At(x, y int) color.Color {
	*m.reads++
	return m.Image.At(x, y)
}

func TestScaleBiLinearReadsTheSourceOnce(t *testing.T) {
	for _, size := range []struct{ source, target image.Rectangle }{
		{image.Rect(0, 0, 300, 200), image.Rect(0, 0, 200, 133)}, // rows of the target share rows of the source
		{image.Rect(0, 0, 64, 48), image.Rect(0, 0, 128, 96)},    // and each takes the same two
	} {
		reads := 0
		scaleBiLinear(image.NewNRGBA(size.target), countingImage{image.NewNRGBA(size.source), &reads})
		if pixels := size.source.Dx() * size.source.Dy(); reads > pixels {
			t.Errorf("scaling %v to %v read %d pixels of the source's %d", size.source.Size(), size.target.Size(), reads, pixels)
		}
	}
}

func TestOpaqueRGBAViewsOnlyOpaquePixels(t *testing.T) {
	m := image.NewNRGBA(image.Rect(2, 3, 5, 7))
	for i := range m.Pix {
		m.Pix[i] = 0xff
	}
	view, ok := opaqueRGBA(m).(*image.RGBA)
	if !ok || &view.Pix[0] != &m.Pix[0] || view.Stride != m.Stride || view.Rect != m.Rect {
		t.Fatalf("opaqueRGBA of an opaque NRGBA = %#v, want an RGBA of its bytes", opaqueRGBA(m))
	}
	m.Pix[len(m.Pix)-1] = 0xfe
	if got := opaqueRGBA(m); got != image.Image(m) {
		t.Fatalf("opaqueRGBA of a translucent NRGBA = %T, want it as it is", got)
	}
	gray := image.NewGray(m.Rect)
	if got := opaqueRGBA(gray); got != image.Image(gray) {
		t.Fatalf("opaqueRGBA of a Gray = %T, want it as it is", got)
	}
}

func TestViewImageScalesAJPEGAsDrawWouldWithoutACallPerPixel(t *testing.T) {
	random := rand.New(rand.NewPCG(5, 6))
	photo := image.NewYCbCr(image.Rect(0, 0, 640, 480), image.YCbCrSubsampleRatio420)
	for i := range photo.Y {
		photo.Y[i] = uint8(i%640/3 + int(random.UintN(40)))
	}
	for i := range photo.Cb {
		photo.Cb[i], photo.Cr[i] = uint8(100+random.UintN(50)), uint8(120+random.UintN(30))
	}
	var source bytes.Buffer
	if err := jpeg.Encode(&source, photo, &jpeg.Options{Quality: 85}); err != nil {
		t.Fatal(err)
	}
	config := ViewImageConfig{MaxSize: 1 << 20, MaxWidth: 320, MaxHeight: 320}
	result, err := prepareViewImage(context.Background(), [][]byte{source.Bytes()}, config, nil)
	if err != nil {
		t.Fatal(err)
	}

	decoded, err := jpeg.Decode(bytes.NewReader(source.Bytes()))
	if err != nil {
		t.Fatal(err)
	}
	resized := image.NewNRGBA(image.Rect(0, 0, 320, 240))
	draw.BiLinear.Scale(resized, resized.Bounds(), decoded, decoded.Bounds(), draw.Src, nil)
	var want bytes.Buffer
	if err := jpeg.Encode(&want, resized, &jpeg.Options{Quality: 90}); err != nil {
		t.Fatal(err)
	}
	if result.EncodedMIMEType != "image/jpeg" || result.Content != base64.StdEncoding.EncodeToString(want.Bytes()) {
		t.Fatalf("the JPEG is %s of %d base64 bytes, want image/jpeg of the %d bytes draw and image/jpeg make",
			result.EncodedMIMEType, len(result.Content), base64.StdEncoding.EncodedLen(want.Len()))
	}

	// Encoding the NRGBA as it is takes an allocation a pixel: 76,800.
	allocations := testing.AllocsPerRun(3, func() {
		if _, err := prepareViewImage(context.Background(), [][]byte{source.Bytes()}, config, nil); err != nil {
			t.Fatal(err)
		}
	})
	if allocations > 2000 {
		t.Errorf("preparing a 640×480 JPEG shown 320 wide made %.0f allocations, want at most 2000", allocations)
	}
}
