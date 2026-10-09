package primitives

import (
	"bytes"
	"fmt"
)

type remoteOutputFrame struct {
	offset int64
	data   []byte
}

type remoteSSEFramer struct {
	maximumFrameSize   int64
	frameDelimiter     SSEFrameDelimiter
	frameWireSize      int64
	emittedOffset      int64
	frame              []byte
	delimiterStart     int
	lineHasData        bool
	afterCR            bool
	frameEndedAtCR     bool
	endedFrameWireSize int64
}

func newRemoteSSEFramer(maximumFrameSize int64, frameDelimiter SSEFrameDelimiter) *remoteSSEFramer {
	return &remoteSSEFramer{
		maximumFrameSize: maximumFrameSize,
		frameDelimiter:   frameDelimiter,
	}
}

func (framer *remoteSSEFramer) append(encoded []byte) ([]remoteOutputFrame, error) {
	var frames []remoteOutputFrame
	for len(encoded) != 0 {
		if framer.afterCR {
			if encoded[0] == '\n' {
				if framer.frameEndedAtCR {
					if framer.endedFrameWireSize >= framer.maximumFrameSize {
						return frames, framer.frameSizeError()
					}
					framer.endedFrameWireSize++
					if framer.frameDelimiter == SSEFrameDelimiterPassThrough {
						if len(frames) != 0 {
							last := &frames[len(frames)-1]
							last.data = append(last.data, encoded[0])
						} else {
							frames = append(frames, remoteOutputFrame{
								offset: framer.emittedOffset,
								data:   []byte{encoded[0]},
							})
						}
						framer.emittedOffset++
					}
					framer.afterCR = false
					framer.frameEndedAtCR = false
					framer.endedFrameWireSize = 0
					encoded = encoded[1:]
					continue
				}
				if err := framer.appendFrameBytes(encoded[:1]); err != nil {
					return frames, err
				}
				framer.afterCR = false
				encoded = encoded[1:]
				continue
			}
			framer.afterCR = false
			framer.frameEndedAtCR = false
			framer.endedFrameWireSize = 0
		}

		lineEnding := bytes.IndexAny(encoded, "\r\n")
		if lineEnding < 0 {
			if err := framer.appendFrameBytes(encoded); err != nil {
				return frames, err
			}
			framer.lineHasData = true
			break
		}
		segment := encoded[:lineEnding+1]
		if err := framer.appendFrameBytes(segment); err != nil {
			return frames, err
		}
		if lineEnding != 0 {
			framer.lineHasData = true
		}
		character := segment[lineEnding]
		encoded = encoded[lineEnding+1:]
		switch character {
		case '\r':
			framer.frameEndedAtCR = !framer.lineHasData
			if framer.lineHasData {
				framer.delimiterStart = len(framer.frame) - 1
			}
			framer.lineHasData = false
			framer.afterCR = true
			if framer.frameEndedAtCR {
				framer.endedFrameWireSize = framer.frameWireSize
				frames = append(frames, framer.takeFrame(framer.delimiterStart))
			}
		case '\n':
			if framer.lineHasData {
				framer.delimiterStart = len(framer.frame) - 1
			} else {
				frames = append(frames, framer.takeFrame(framer.delimiterStart))
			}
			framer.lineHasData = false
		default:
			framer.lineHasData = true
		}
	}
	return frames, nil
}

func (framer *remoteSSEFramer) finish() []remoteOutputFrame {
	framer.afterCR = false
	framer.frameEndedAtCR = false
	framer.endedFrameWireSize = 0
	if len(framer.frame) == 0 || framer.frameDelimiter == SSEFrameDelimiterStrip {
		framer.frameWireSize = 0
		framer.frame = nil
		framer.delimiterStart = 0
		return nil
	}
	return []remoteOutputFrame{framer.takeFrame(len(framer.frame))}
}

func (framer *remoteSSEFramer) appendFrameBytes(encoded []byte) error {
	size := int64(len(encoded))
	if size > framer.maximumFrameSize-framer.frameWireSize {
		return framer.frameSizeError()
	}
	framer.frame = append(framer.frame, encoded...)
	framer.frameWireSize += size
	return nil
}

func (framer *remoteSSEFramer) takeFrame(delimiterStart int) remoteOutputFrame {
	data := framer.frame
	if framer.frameDelimiter == SSEFrameDelimiterStrip {
		data = data[:delimiterStart]
	}
	frame := remoteOutputFrame{offset: framer.emittedOffset, data: data}
	framer.emittedOffset += int64(len(data))
	framer.frameWireSize = 0
	framer.frame = nil
	framer.delimiterStart = 0
	return frame
}

func (framer *remoteSSEFramer) frameSizeError() error {
	return fmt.Errorf("remote SSE frame exceeds %d bytes", framer.maximumFrameSize)
}
