package main

import (
	"log"
	"os"
	"slices"
)

// A run's events are kept in a file, not in memory. A long run publishes
// tens of megabytes of them, an entry going out whole each time it changes,
// and they are read again only by browsers that open the session while it
// runs, or that reconnect. The file is removed as soon as it is made, so it
// goes with the server however that ends; the log keeps where in it each
// event is. An event that cannot be written, for want of a file or of room,
// is kept in memory instead.
type eventLog struct {
	file  *os.File
	name  string // the file's name, where an open file cannot be removed: it is removed when closed
	tried bool   // whether a file was made, or could not be
	size  int64  // how much the file holds
	index []logged
}

// logged is where an event of a log is.
type logged struct {
	at   int64 // where in the file it starts
	n    int   // how long it is
	data []byte
}

// readSize is about how much of the file is read at once.
const readSize = 1 << 20

// append adds an event to the log.
func (l *eventLog) append(data []byte) {
	if !l.tried {
		l.tried = true
		l.open()
	}
	if l.file != nil {
		if _, err := l.file.WriteAt(data, l.size); err == nil {
			l.index = append(l.index, logged{at: l.size, n: len(data)})
			l.size += int64(len(data))
			return
		}
	}
	l.index = append(l.index, logged{data: data})
}

func (l *eventLog) open() {
	file, err := os.CreateTemp("", "kou-conveyor-run-*.events")
	if err != nil {
		log.Printf("a run's events stay in memory: %v", err)
		return
	}
	l.file = file
	if os.Remove(file.Name()) != nil {
		l.name = file.Name()
	}
}

// each calls fn with each of events, a part of the log, in turn; fn has an
// event only until it returns. Events that follow each other in the file
// are read at once.
func (l *eventLog) each(events []logged, fn func(data []byte) error) error {
	var buffer []byte
	for i := 0; i < len(events); {
		first := events[i]
		if first.data != nil {
			if err := fn(first.data); err != nil {
				return err
			}
			i++
			continue
		}
		j, end := i+1, first.at+int64(first.n)
		for j < len(events) && events[j].data == nil && events[j].at == end && end-first.at < readSize {
			end += int64(events[j].n)
			j++
		}
		buffer = slices.Grow(buffer[:0], int(end-first.at))[:end-first.at]
		if _, err := l.file.ReadAt(buffer, first.at); err != nil {
			return err
		}
		for _, e := range events[i:j] {
			at := e.at - first.at
			if err := fn(buffer[at : at+int64(e.n)]); err != nil {
				return err
			}
		}
		i = j
	}
	return nil
}

// close lets go of the file, and of the events in it.
func (l *eventLog) close() {
	if l.file == nil {
		return
	}
	l.file.Close()
	if l.name != "" {
		os.Remove(l.name)
	}
	l.file = nil
}
