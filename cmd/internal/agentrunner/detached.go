package agentrunner

import (
	"errors"
	"io"
	"time"
)

// followPoll is how often a detached run looks for what its front-end
// appended to stdin.
const followPoll = 50 * time.Millisecond

// followReader reads a file that grows, as tail -f does: at its end it
// waits for more, until done is closed. A detached runner's stdin is such a
// file (-detached), which the front-end appends the messages for the
// running agent to, and the stop line.
type followReader struct {
	r    io.Reader
	done <-chan struct{}
}

func (f *followReader) Read(p []byte) (int, error) {
	if len(p) == 0 {
		return 0, nil
	}
	for {
		n, err := f.r.Read(p)
		if errors.Is(err, io.EOF) {
			err = nil
		}
		if n > 0 || err != nil {
			return n, err
		}
		select {
		case <-f.done:
			return 0, io.EOF
		case <-time.After(followPoll):
		}
	}
}
