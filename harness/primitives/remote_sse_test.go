package primitives

import (
	"bytes"
	"fmt"
	"reflect"
	"testing"
)

func TestRemoteSSEFramerEmitsCompleteFramesAcrossChunks(t *testing.T) {
	chunks := [][]byte{
		[]byte("data: one\r"),
		[]byte("\n\r"),
		[]byte("\ndata: two\n\n: keepalive\r\rpartial"),
	}
	framer := newRemoteSSEFramer(1024, SSEFrameDelimiterPassThrough)
	var frames []remoteOutputFrame
	for _, chunk := range chunks {
		decoded, err := framer.append(chunk)
		if err != nil {
			t.Fatal(err)
		}
		frames = append(frames, decoded...)
	}
	want := [][]byte{
		[]byte("data: one\r\n\r"),
		[]byte("\n"),
		[]byte("data: two\n\n"),
		[]byte(": keepalive\r\r"),
	}
	if len(frames) != len(want) {
		t.Fatalf("frames = %#v", frames)
	}
	var offset int64
	for index, frame := range frames {
		if frame.offset != offset || !bytes.Equal(frame.data, want[index]) {
			t.Fatalf("frame %d = (%d, %q), want (%d, %q)", index, frame.offset, frame.data, offset, want[index])
		}
		offset += int64(len(frame.data))
	}
}

func TestRemoteSSEFramerEmitsFrameEndingInBareCR(t *testing.T) {
	framer := newRemoteSSEFramer(1024, SSEFrameDelimiterPassThrough)
	frames, err := framer.append([]byte("data: one\r\r"))
	if err != nil {
		t.Fatal(err)
	}
	if len(frames) != 1 || frames[0].offset != 0 || !bytes.Equal(frames[0].data, []byte("data: one\r\r")) {
		t.Fatalf("frames = %#v", frames)
	}
}

func TestRemoteSSEFramerPreservesIncompleteBytesAtEOF(t *testing.T) {
	framer := newRemoteSSEFramer(1024, SSEFrameDelimiterPassThrough)
	frames, err := framer.append([]byte("data: partial"))
	if err != nil {
		t.Fatal(err)
	}
	if len(frames) != 0 {
		t.Fatalf("frames before EOF = %#v", frames)
	}
	frames = framer.finish()
	if len(frames) != 1 || frames[0].offset != 0 || !bytes.Equal(frames[0].data, []byte("data: partial")) {
		t.Fatalf("frames = %#v", frames)
	}
}

func TestRemoteSSEFramerDoesNotEmitIncompleteFrameWhenStrippingDelimiter(t *testing.T) {
	framer := newRemoteSSEFramer(1024, SSEFrameDelimiterStrip)
	frames, err := framer.append([]byte("data: partial"))
	if err != nil {
		t.Fatal(err)
	}
	frames = append(frames, framer.finish()...)
	if len(frames) != 0 {
		t.Fatalf("frames = %#v", frames)
	}
}

func TestRemoteSSEFramerEmitsEveryEmptyFrameWhenStrippingDelimiter(t *testing.T) {
	framer := newRemoteSSEFramer(1024, SSEFrameDelimiterStrip)
	frames, err := framer.append([]byte("\n\ndata: x\n\n"))
	if err != nil {
		t.Fatal(err)
	}
	want := []remoteOutputFrame{
		{offset: 0, data: []byte{}},
		{offset: 0, data: []byte{}},
		{offset: 0, data: []byte("data: x")},
	}
	if !reflect.DeepEqual(frames, want) {
		t.Fatalf("frames = %#v, want %#v", frames, want)
	}
}

func TestRemoteSSEFramerBoundsIncompleteFrame(t *testing.T) {
	framer := newRemoteSSEFramer(4, SSEFrameDelimiterStrip)
	if _, err := framer.append([]byte("1234")); err != nil {
		t.Fatal(err)
	}
	if _, err := framer.append([]byte("5")); err == nil {
		t.Fatal("expected response size failure")
	}
}

func TestRemoteSSEFramerPassesThroughEveryNewlineBoundary(t *testing.T) {
	newlines := []string{"\n", "\r", "\r\n"}
	for _, lineEnding := range newlines {
		for _, blankLine := range newlines {
			if lineEnding == "\r" && blankLine == "\n" {
				continue
			}
			name := fmt.Sprintf("line=%q/blank=%q", lineEnding, blankLine)
			t.Run(name, func(t *testing.T) {
				encoded := []byte("data: one" + lineEnding + blankLine + "data: two" + lineEnding + blankLine)
				for cut := range len(encoded) + 1 {
					framer := newRemoteSSEFramer(int64(len(encoded)), SSEFrameDelimiterPassThrough)
					var frames []remoteOutputFrame
					for _, chunk := range [][]byte{encoded[:cut], encoded[cut:]} {
						decoded, err := framer.append(chunk)
						if err != nil {
							t.Fatalf("cut %d: %v", cut, err)
						}
						frames = append(frames, decoded...)
					}
					frames = append(frames, framer.finish()...)

					var output []byte
					var offset int64
					for _, frame := range frames {
						if frame.offset != offset {
							t.Fatalf("cut %d: offset = %d, want %d", cut, frame.offset, offset)
						}
						output = append(output, frame.data...)
						offset += int64(len(frame.data))
					}
					if !bytes.Equal(output, encoded) {
						t.Fatalf("cut %d: output = %q, want %q", cut, output, encoded)
					}
				}
			})
		}
	}
}

func TestRemoteSSEFramerStripsEveryFrameDelimiter(t *testing.T) {
	newlines := []string{"\n", "\r", "\r\n"}
	for _, lineEnding := range newlines {
		for _, blankLine := range newlines {
			if lineEnding == "\r" && blankLine == "\n" {
				continue
			}
			name := fmt.Sprintf("line=%q/blank=%q", lineEnding, blankLine)
			t.Run(name, func(t *testing.T) {
				encoded := []byte("data: one" + lineEnding + blankLine + "data: two" + lineEnding + blankLine)
				for cut := range len(encoded) + 1 {
					framer := newRemoteSSEFramer(int64(len(encoded)), SSEFrameDelimiterStrip)
					var frames []remoteOutputFrame
					for _, chunk := range [][]byte{encoded[:cut], encoded[cut:]} {
						decoded, err := framer.append(chunk)
						if err != nil {
							t.Fatalf("cut %d: %v", cut, err)
						}
						frames = append(frames, decoded...)
					}
					frames = append(frames, framer.finish()...)
					want := []remoteOutputFrame{
						{offset: 0, data: []byte("data: one")},
						{offset: int64(len("data: one")), data: []byte("data: two")},
					}
					if !reflect.DeepEqual(frames, want) {
						t.Fatalf("cut %d: frames = %#v, want %#v", cut, frames, want)
					}
				}
			})
		}
	}
}

func TestRemoteSSEFramerReturnsCompleteFramesBeforeSizeFailure(t *testing.T) {
	framer := newRemoteSSEFramer(5, SSEFrameDelimiterStrip)
	frames, err := framer.append([]byte("one\n\n123456"))
	if err == nil {
		t.Fatal("expected frame size failure")
	}
	if len(frames) != 1 || frames[0].offset != 0 || !bytes.Equal(frames[0].data, []byte("one")) {
		t.Fatalf("frames = %#v", frames)
	}
}

func TestRemoteSSEFramerCountsSplitCRLFInFrameSize(t *testing.T) {
	framer := newRemoteSSEFramer(5, SSEFrameDelimiterStrip)
	frames, err := framer.append([]byte("one\n\r"))
	if err != nil {
		t.Fatal(err)
	}
	if len(frames) != 1 || !bytes.Equal(frames[0].data, []byte("one")) {
		t.Fatalf("frames = %#v", frames)
	}
	if _, err := framer.append([]byte("\n")); err == nil {
		t.Fatal("expected frame size failure")
	}
}

func BenchmarkRemoteSSEFramer(b *testing.B) {
	frame := append([]byte("data: "), bytes.Repeat([]byte{'x'}, 120)...)
	frame = append(frame, '\n', '\n')
	encoded := bytes.Repeat(frame, 256)

	b.SetBytes(int64(len(encoded)))
	b.ReportAllocs()
	for b.Loop() {
		framer := newRemoteSSEFramer(int64(len(frame)), SSEFrameDelimiterStrip)
		frames, err := framer.append(encoded)
		if err != nil {
			b.Fatal(err)
		}
		if len(frames) != 256 {
			b.Fatalf("frames = %d", len(frames))
		}
	}
}
