package main

import (
	"bytes"
	"encoding/json/v2"
	"errors"
	"fmt"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// A run's events are read back from the file as they were published, a part
// of the log or all of it, however long each is; a log without a file keeps
// them in memory.
func TestEventLogKeepsEventsInAFile(t *testing.T) {
	var l eventLog
	defer l.close()
	var want [][]byte
	for i := range 300 {
		size := 10 + i*97
		if i%50 == 7 {
			size = 3 * readSize // longer than a read
		}
		event := fmt.Appendf(nil, "%d:%s", i, strings.Repeat("x", size))
		l.append(event, nil)
		want = append(want, event)
	}
	if l.file == nil {
		t.Fatal("the log has no file")
	}
	for _, e := range l.index {
		if e.data != nil {
			t.Fatal("an event is in memory")
		}
	}
	for _, part := range [][2]int{{0, 300}, {0, 1}, {7, 8}, {150, 300}, {299, 300}} {
		var got [][]byte
		err := l.each(l.index[part[0]:part[1]], func(i int, data []byte) error {
			if i != len(got) {
				t.Fatalf("events %v: event %d said it is %d", part, len(got), i)
			}
			got = append(got, bytes.Clone(data))
			return nil
		})
		if err != nil || len(got) != part[1]-part[0] {
			t.Fatalf("events %v: %d, %v", part, len(got), err)
		}
		for i := range got {
			if !bytes.Equal(got[i], want[part[0]+i]) {
				t.Fatalf("event %d differs", part[0]+i)
			}
		}
	}
	// What fn returns stops the reading.
	stop, n := errors.New("stop"), 0
	if err := l.each(l.index, func(int, []byte) error { n++; return stop }); err != stop || n != 1 {
		t.Fatalf("each went on after an error: %d events, %v", n, err)
	}
	l.close()
	if l.file != nil {
		t.Fatal("the file is still open")
	}

	memory := eventLog{tried: true} // no file could be made
	memory.append([]byte("a"), nil)
	memory.append([]byte("b"), nil)
	var got []string
	if err := memory.each(memory.index, func(_ int, data []byte) error { got = append(got, string(data)); return nil }); err != nil || strings.Join(got, ",") != "a,b" {
		t.Fatalf("events in memory = %q, %v", got, err)
	}
}

// A browser that reads a run's log after a preview was recorded whole, or
// after a tool call ended, gets the entry without the pieces that streamed
// of it; the events it gets keep their IDs.
func TestEventLogSkipsTheStreamsThatAreOver(t *testing.T) {
	r := &run{wake: make(chan struct{})}
	defer r.events.close()
	preview := "stream:t1.1"
	r.publish(event{Type: "delta", Delta: &cockpit.Delta{ID: preview, Kind: cockpit.KindAssistant, Text: "Hel"}})
	r.publish(event{Type: "output", Output: &cockpit.Output{ID: "tool:c1", Stream: "out", Text: "one\n"}})
	r.publish(event{Type: "delta", Delta: &cockpit.Delta{ID: preview, Kind: cockpit.KindAssistant, Text: "lo"}})
	read := func() []string {
		events, _, _ := r.since(0)
		var got []string
		err := r.events.each(events, func(i int, data []byte) error {
			var e event
			if err := json.Unmarshal(data, &e); err != nil {
				return err
			}
			switch {
			case e.Delta != nil:
				got = append(got, fmt.Sprintf("%d:delta:%s", i+1, e.Delta.Text))
			case e.Output != nil:
				got = append(got, fmt.Sprintf("%d:output:%s", i+1, strings.TrimSpace(e.Output.Text)))
			case e.Entry != nil:
				got = append(got, fmt.Sprintf("%d:entry:%s", i+1, e.Entry.ID))
			default:
				got = append(got, fmt.Sprintf("%d:%s", i+1, e.Type))
			}
			return nil
		})
		if err != nil {
			t.Fatal(err)
		}
		return got
	}
	if got := strings.Join(read(), " "); got != "1:delta:Hel 2:output:one 3:delta:lo" {
		t.Fatalf("while it streams: %s", got)
	}
	r.publish(event{Type: "entry", Entry: &cockpit.Entry{ID: "output:4.0", Kind: cockpit.KindAssistant, Text: "Hello", Replaces: preview}})
	if got := strings.Join(read(), " "); got != "2:output:one 4:entry:output:4.0" {
		t.Fatalf("once the preview is recorded: %s", got)
	}
	r.publish(event{Type: "entry", Entry: &cockpit.Entry{ID: "tool:c1", Kind: cockpit.KindTool, Tool: &cockpit.Tool{CallID: "c1", State: cockpit.ToolDone}}})
	r.publish(event{Type: "status", Activity: "Thinking"})
	if got := strings.Join(read(), " "); got != "4:entry:output:4.0 5:entry:tool:c1 6:status" {
		t.Fatalf("once the tool call ended: %s", got)
	}
	if len(r.streams) != 0 {
		t.Fatalf("streams kept after they ended: %v", r.streams)
	}
}

// A run's events go once the run is gone from the server and the last
// browser that reads them is done.
func TestRetiredRunLetsItsEventsGo(t *testing.T) {
	r := &run{wake: make(chan struct{})}
	r.publish(event{Type: "log", Text: "one"})
	if !r.attach() {
		t.Fatal("a run could not be read")
	}
	r.retire()
	if r.attach() {
		t.Fatal("a run that is gone was read")
	}
	events, _, _ := r.since(0)
	var got []string
	err := r.events.each(events, func(_ int, data []byte) error { got = append(got, string(data)); return nil })
	if err != nil || len(got) != 1 || !strings.Contains(got[0], `"one"`) {
		t.Fatalf("events while read = %q, %v", got, err)
	}
	r.detach()
	if r.events.file != nil {
		t.Fatal("the log outlived its last reader")
	}
}
