package localfile

import (
	"encoding/binary"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"strconv"
	"strings"
	"sync"
	"sync/atomic"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

// resident is how much of the process's memory is resident, in KiB, as ps
// and top show it.
func resident(t *testing.T) int64 {
	t.Helper()
	output, err := exec.Command("ps", "-o", "rss=", "-p", strconv.Itoa(os.Getpid())).Output()
	if err != nil {
		t.Fatal(err)
	}
	kib, err := strconv.ParseInt(strings.TrimSpace(string(output)), 10, 64)
	if err != nil {
		t.Fatal(err)
	}
	return kib
}

// mapOffsets maps a file of size bytes, each word of which is its offset.
func mapOffsets(t *testing.T, size int) ([]byte, *mapping) {
	t.Helper()
	path := filepath.Join(t.TempDir(), "log")
	file, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	chunk := make([]byte, 1<<20)
	for offset := 0; offset < size; offset += len(chunk) {
		for at := 0; at < len(chunk); at += 8 {
			binary.LittleEndian.PutUint64(chunk[at:], uint64(offset+at))
		}
		if _, err := file.Write(chunk); err != nil {
			t.Fatal(err)
		}
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}
	mapped, held, err := mapFile(path)
	if err != nil {
		t.Fatal(err)
	}
	if held == nil {
		t.Fatal("the file was read, not mapped")
	}
	t.Cleanup(held.close)
	return mapped, held
}

// A mapping read through counts in the process's resident memory, as ps and
// top show it, though its pages are the file's. Giving them back takes them
// out of it, and the mapping reads the file still, but for a page written
// to, which keeps what was written.
func TestDropTakesTheMappingOutOfResidentMemory(t *testing.T) {
	const size = 64 << 20
	mapped, held := mapOffsets(t, size)
	start := resident(t)
	for offset := 0; offset < len(mapped); offset += os.Getpagesize() {
		if word := binary.LittleEndian.Uint64(mapped[offset:]); word != uint64(offset) {
			t.Fatalf("the mapping reads %d at %d", word, offset)
		}
	}
	read := resident(t)
	if read-start < size/2/1024 {
		t.Skipf("reading a mapping of %d KiB through took resident memory from %d KiB only to %d", size/1024, start, read)
	}
	const written = size/2 + 8
	binary.LittleEndian.PutUint64(mapped[written:], 1)
	held.drop()
	released := resident(t)
	if read-released < (read-start)*3/4 {
		t.Fatalf(
			"resident memory went from %d KiB to %d as a mapping of %d KiB read through was given back, from %d before it was read",
			read, released, size/1024, start,
		)
	}
	for offset := 0; offset < size; offset += 8 {
		want := uint64(offset)
		if offset == written {
			want = 1
		}
		if word := binary.LittleEndian.Uint64(mapped[offset:]); word != want {
			t.Fatalf("once its pages are given back the mapping reads %d at %d", word, offset)
		}
	}
}

// Those reading a mapping as its pages are given back, over and over, read
// the file all along.
func TestDropKeepsTheMappingReadable(t *testing.T) {
	const size = 16 << 20
	mapped, held := mapOffsets(t, size)
	step := os.Getpagesize() / 4
	var (
		readers sync.WaitGroup
		stop    atomic.Bool
		passes  atomic.Int64
	)
	failures := make(chan string, 4)
	for reader := range 4 {
		readers.Go(func() {
			for !stop.Load() {
				for offset := reader * 8; offset < size; offset += step {
					if word := binary.LittleEndian.Uint64(mapped[offset:]); word != uint64(offset) {
						failures <- fmt.Sprintf("reader %d read %d at %d", reader, word, offset)
						return
					}
				}
				passes.Add(1)
			}
		})
	}
	drops := 0
	for start := time.Now(); time.Since(start) < time.Second/2 ||
		passes.Load() < 40 && len(failures) == 0 && time.Since(start) < 10*time.Second; drops++ {
		held.drop()
	}
	stop.Store(true)
	readers.Wait()
	close(failures)
	for failure := range failures {
		t.Error(failure)
	}
	t.Logf("%d passes of 4 readers through %d drops", passes.Load(), drops)
}

// A store that maps session files gives their pages back once it has read
// them: reading a session leaves a small part of its size resident.
func TestStoreMappingLogsLeavesThemOutOfResidentMemory(t *testing.T) {
	directory := t.TempDir()
	writer, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	writeFinishedCall(t, writer, strings.Repeat("output ", 5<<20))
	info, err := os.Stat(writer.sessionPath("session-1"))
	if err != nil {
		t.Fatal(err)
	}
	read, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	want, err := read.Items(t.Context(), "session-1", sessionstore.BeforeFirst, 10)
	if err != nil {
		t.Fatal(err)
	}
	store, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	store.MapLogs()
	start := resident(t)
	page, err := store.Items(t.Context(), "session-1", sessionstore.BeforeFirst, 10)
	if err != nil {
		t.Fatal(err)
	}
	if grown := resident(t) - start; grown > info.Size()/4/1024 {
		t.Fatalf("reading a mapped session of %d KiB took resident memory from %d KiB up by %d", info.Size()/1024, start, grown)
	}
	if !reflect.DeepEqual(page, want) {
		t.Fatalf("mapped session's items = %#v, read one's = %#v", page, want)
	}
}

// What a store decoded from a mapped session file reads the file's pages
// through again as it is read, as a run restoring its session reads the
// outputs of all its tool calls. The store gives them back again as it lets
// go of it, at its first append.
func TestStoreGivesPagesBackAsItLetsGoOfASession(t *testing.T) {
	directory := t.TempDir()
	writer, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	writeFinishedCall(t, writer, strings.Repeat("output ", 5<<20))
	info, err := os.Stat(writer.sessionPath("session-1"))
	if err != nil {
		t.Fatal(err)
	}
	read, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	want, err := read.Items(t.Context(), "session-1", sessionstore.BeforeFirst, 10)
	if err != nil {
		t.Fatal(err)
	}
	store, err := New(directory)
	if err != nil {
		t.Fatal(err)
	}
	store.MapLogs()
	page, err := store.Items(t.Context(), "session-1", sessionstore.BeforeFirst, 10)
	if err != nil {
		t.Fatal(err)
	}
	start := resident(t)
	if !reflect.DeepEqual(page, want) {
		t.Fatal("the mapped session's items differ from the read one's")
	}
	through := resident(t)
	if through-start < info.Size()/2/1024 {
		t.Skipf("reading a mapped session of %d KiB through took resident memory from %d KiB only to %d", info.Size()/1024, start, through)
	}
	if err := store.AppendTurn(t.Context(), "session-1", session.Turn{
		ID: "turn-2", PreviousTurnID: "turn-1", Type: session.TurnRegular,
	}); err != nil {
		t.Fatal(err)
	}
	if after := resident(t); through-after < (through-start)/2 {
		t.Fatalf(
			"resident memory went from %d KiB to %d as the store let go of a session of %d KiB read through, from %d before",
			through, after, info.Size()/1024, start,
		)
	}
	if !reflect.DeepEqual(page, want) {
		t.Fatal("once the store let go of them, the mapped session's items differ from the read one's")
	}
}
