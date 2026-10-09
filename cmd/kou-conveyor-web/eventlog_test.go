package main

import (
	"bytes"
	"errors"
	"fmt"
	"strings"
	"testing"
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
		l.append(event)
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
		err := l.each(l.index[part[0]:part[1]], func(data []byte) error {
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
	if err := l.each(l.index, func([]byte) error { n++; return stop }); err != stop || n != 1 {
		t.Fatalf("each went on after an error: %d events, %v", n, err)
	}
	l.close()
	if l.file != nil {
		t.Fatal("the file is still open")
	}

	memory := eventLog{tried: true} // no file could be made
	memory.append([]byte("a"))
	memory.append([]byte("b"))
	var got []string
	if err := memory.each(memory.index, func(data []byte) error { got = append(got, string(data)); return nil }); err != nil || strings.Join(got, ",") != "a,b" {
		t.Fatalf("events in memory = %q, %v", got, err)
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
	err := r.events.each(events, func(data []byte) error { got = append(got, string(data)); return nil })
	if err != nil || len(got) != 1 || !strings.Contains(got[0], `"one"`) {
		t.Fatalf("events while read = %q, %v", got, err)
	}
	r.detach()
	if r.events.file != nil {
		t.Fatal("the log outlived its last reader")
	}
}
