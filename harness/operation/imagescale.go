package operation

import (
	"image"
	"image/color"
	"math"
)

// scaleBiLinear draws src, scaled, over all of dst: the pixels that
//
//	draw.BiLinear.Scale(dst, dst.Bounds(), src, src.Bounds(), draw.Src, nil)
//
// of golang.org/x/image/draw draws, computed as it computes them, operation
// for operation — the code below is draw's kernel scaler cut down to this one
// case — but in a fraction of the memory. draw first scales every row of the
// source to the width of dst and keeps them all, 32 bytes a pixel: 120 MB for
// a 2720×1880 screenshot shown 2000 wide, 184 MB for a 5K one. This keeps only
// the rows the row of dst it draws is made of: a handful, as the tent spans
// twice the scale.
func scaleBiLinear(dst *image.NRGBA, src image.Image) {
	sr, dr := src.Bounds(), dst.Bounds()
	if sr.Empty() || dr.Empty() {
		return
	}
	dw := dr.Dx()
	horizontal := newBiLinearDistrib(int32(dw), int32(sr.Dx()))
	vertical := newBiLinearDistrib(int32(dr.Dy()), int32(sr.Dy()))
	load, opaque := biLinearRows(src)

	// rows holds rows of the source scaled to the width of dst, the row y of
	// the source in slot y%span once held[slot] is y.
	span := 1
	for _, s := range vertical.sources {
		if s.i < s.j {
			span = max(span, int(vertical.contribs[s.j-1].coord-vertical.contribs[s.i].coord)+1)
		}
	}
	rows := make([][4]float64, span*dw)
	held := make([]int32, span)
	for slot := range held {
		held[slot] = -1
	}
	pixels := make([]float64, 4*sr.Dx())
	for dy, s := range vertical.sources {
		contribs := vertical.contribs[s.i:s.j]
		for _, c := range contribs {
			slot := int(c.coord) % span
			if held[slot] == c.coord {
				continue
			}
			held[slot] = c.coord
			load(sr.Min.Y+int(c.coord), pixels)
			row := rows[slot*dw : (slot+1)*dw]
			for x, h := range horizontal.sources {
				var pr, pg, pb, pa float64
				for _, k := range horizontal.contribs[h.i:h.j] {
					p := pixels[4*k.coord : 4*k.coord+4 : 4*k.coord+4]
					pr += float64(p[0] * k.weight)
					pg += float64(p[1] * k.weight)
					pb += float64(p[2] * k.weight)
					pa += float64(p[3] * k.weight)
				}
				alpha := pa * h.invTotalWeightFFFF
				if opaque {
					alpha = 1
				}
				row[x] = [4]float64{
					pr * h.invTotalWeightFFFF,
					pg * h.invTotalWeightFFFF,
					pb * h.invTotalWeightFFFF,
					alpha,
				}
			}
		}
		for dx := range dw {
			var pr, pg, pb, pa float64
			for _, c := range contribs {
				p := &rows[int(c.coord)%span*dw+dx]
				pr += float64(p[0] * c.weight)
				pg += float64(p[1] * c.weight)
				pb += float64(p[2] * c.weight)
				pa += float64(p[3] * c.weight)
			}
			if pr > pa {
				pr = pa
			}
			if pg > pa {
				pg = pa
			}
			if pb > pa {
				pb = pa
			}
			dst.SetRGBA64(dr.Min.X+dx, dr.Min.Y+dy, color.RGBA64{
				R: biLinearFtou(pr * s.invTotalWeight),
				G: biLinearFtou(pg * s.invTotalWeight),
				B: biLinearFtou(pb * s.invTotalWeight),
				A: biLinearFtou(pa * s.invTotalWeight),
			})
		}
	}
}

// biLinearRows returns what reads the row y of src into pixels, four values
// a pixel as draw's scaler takes them (16-bit, alpha-premultiplied), and
// whether draw takes the alpha of src as 1 rather than from its pixels, as it
// does for its gray and YCbCr images.
func biLinearRows(src image.Image) (func(y int, pixels []float64), bool) {
	b := src.Bounds()
	switch src := src.(type) {
	case *image.Gray:
		return func(y int, pixels []float64) {
			row := src.Pix[src.PixOffset(b.Min.X, y):]
			for x := range b.Dx() {
				v := float64(uint32(row[x]) * 0x101)
				pixels[4*x], pixels[4*x+1], pixels[4*x+2], pixels[4*x+3] = v, v, v, 0xffff
			}
		}, true
	case *image.NRGBA:
		return func(y int, pixels []float64) {
			row := src.Pix[src.PixOffset(b.Min.X, y):]
			for x := range b.Dx() {
				p := row[4*x : 4*x+4 : 4*x+4]
				pau := uint32(p[3]) * 0x101
				pixels[4*x] = float64(uint32(p[0]) * pau / 0xff)
				pixels[4*x+1] = float64(uint32(p[1]) * pau / 0xff)
				pixels[4*x+2] = float64(uint32(p[2]) * pau / 0xff)
				pixels[4*x+3] = float64(pau)
			}
		}, false
	case *image.RGBA:
		return func(y int, pixels []float64) {
			row := src.Pix[src.PixOffset(b.Min.X, y):]
			for x := range b.Dx() {
				p := row[4*x : 4*x+4 : 4*x+4]
				pixels[4*x] = float64(uint32(p[0]) * 0x101)
				pixels[4*x+1] = float64(uint32(p[1]) * 0x101)
				pixels[4*x+2] = float64(uint32(p[2]) * 0x101)
				pixels[4*x+3] = float64(uint32(p[3]) * 0x101)
			}
		}, false
	case *image.YCbCr:
		switch src.SubsampleRatio {
		case image.YCbCrSubsampleRatio444, image.YCbCrSubsampleRatio422,
			image.YCbCrSubsampleRatio420, image.YCbCrSubsampleRatio440:
			return func(y int, pixels []float64) {
				for x := range b.Dx() {
					r, g, bl, _ := src.YCbCrAt(b.Min.X+x, y).RGBA()
					pixels[4*x], pixels[4*x+1], pixels[4*x+2], pixels[4*x+3] = float64(r), float64(g), float64(bl), 0xffff
				}
			}, true
		}
		// draw takes the other subsamplings pixel by pixel, as any image.
	case image.RGBA64Image:
		return func(y int, pixels []float64) {
			for x := range b.Dx() {
				c := src.RGBA64At(b.Min.X+x, y)
				pixels[4*x], pixels[4*x+1], pixels[4*x+2], pixels[4*x+3] = float64(c.R), float64(c.G), float64(c.B), float64(c.A)
			}
		}, false
	}
	return func(y int, pixels []float64) {
		for x := range b.Dx() {
			r, g, bl, a := src.At(b.Min.X+x, y).RGBA()
			pixels[4*x], pixels[4*x+1], pixels[4*x+2], pixels[4*x+3] = float64(r), float64(g), float64(bl), float64(a)
		}
	}, false
}

// biLinearDistrib is how the columns (or rows) of the source are spread over
// those of the destination: draw's distrib, of its tent kernel.
type biLinearDistrib struct {
	// sources are the contribs each column of the destination takes, and
	// the inverse of their total weight.
	sources  []biLinearSource
	contribs []biLinearContrib
}

type biLinearSource struct {
	i, j               int32
	invTotalWeight     float64
	invTotalWeightFFFF float64
}

type biLinearContrib struct {
	coord  int32
	weight float64
}

// newBiLinearDistrib spreads sw columns of the source over dw of the
// destination: draw's newDistrib with draw.BiLinear.
func newBiLinearDistrib(dw, sw int32) biLinearDistrib {
	const support = 1 // the tent's
	scale := float64(sw) / float64(dw)
	halfWidth, kernelArgScale := float64(support), 1.0
	// When shrinking, broaden the kernel so that every column of the source
	// is taken.
	if scale > 1 {
		halfWidth *= scale
		kernelArgScale = 1 / scale
	}
	// First each source holds the center of its column in invTotalWeight,
	// and the range of columns of the source it may take in i and j.
	n, sources := int32(0), make([]biLinearSource, dw)
	for x := range sources {
		center := float64((float64(x)+0.5)*scale) - 0.5
		i := int32(math.Floor(center - halfWidth))
		if i < 0 {
			i = 0
		}
		j := int32(math.Ceil(center + halfWidth))
		if j > sw {
			j = sw
			if j < i {
				j = i
			}
		}
		sources[x] = biLinearSource{i: i, j: j, invTotalWeight: center}
		n += j - i
	}
	contribs := make([]biLinearContrib, 0, n)
	for k, b := range sources {
		totalWeight := 0.0
		l := int32(len(contribs))
		for coord := b.i; coord < b.j; coord++ {
			t := biLinearAbs((b.invTotalWeight - float64(coord)) * kernelArgScale)
			if t >= support {
				continue
			}
			weight := float64(1 - t) // the tent: draw.BiLinear.At(t)
			if weight == 0 {
				continue
			}
			totalWeight += weight
			contribs = append(contribs, biLinearContrib{coord, weight})
		}
		totalWeight = 1 / totalWeight
		sources[k] = biLinearSource{
			i:                  l,
			j:                  int32(len(contribs)),
			invTotalWeight:     totalWeight,
			invTotalWeightFFFF: totalWeight / 0xffff,
		}
	}
	return biLinearDistrib{sources, contribs}
}

// biLinearAbs is draw's abs: math.Abs, careless of negative zero, infinities
// and NaNs.
func biLinearAbs(f float64) float64 {
	if f < 0 {
		f = -f
	}
	return f
}

// biLinearFtou is draw's ftou: [0, 1] to [0, 0xffff].
func biLinearFtou(f float64) uint16 {
	i := int32(float64(0xffff*f) + 0.5)
	if i > 0xffff {
		return 0xffff
	}
	if i > 0 {
		return uint16(i)
	}
	return 0
}

// opaqueRGBA gives an opaque *image.NRGBA as the *image.RGBA of its bytes,
// the same pixels when every alpha is 0xff, which image/jpeg encodes from
// its bytes rather than with a call, and an allocation, per pixel.
func opaqueRGBA(m image.Image) image.Image {
	if n, ok := m.(*image.NRGBA); ok && n.Opaque() {
		return &image.RGBA{Pix: n.Pix, Stride: n.Stride, Rect: n.Rect}
	}
	return m
}
