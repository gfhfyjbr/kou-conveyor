package agentrunner

import (
	"bytes"
	"encoding/json/v2"
	"os"
	"path/filepath"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func prefixLines(t *testing.T, path string) []prefixLine {
	t.Helper()
	data, err := os.ReadFile(path)
	if err != nil {
		t.Fatal(err)
	}
	var lines []prefixLine
	for _, raw := range bytes.Split(bytes.TrimSuffix(data, []byte("\n")), []byte("\n")) {
		var line prefixLine
		if err := json.Unmarshal(raw, &line); err != nil {
			t.Fatalf("%v: %s", err, raw)
		}
		lines = append(lines, line)
	}
	return lines
}

func prefixReport(tools string, messages ...string) llm.PrefixReport {
	return llm.PrefixReport{
		Model: "claude-opus-5", Response: "msg_" + messages[len(messages)-1],
		Usage:  llm.Usage{InputTokens: 1000, CachedInputTokens: 900, CacheWriteInputTokens: 50},
		Prefix: llm.Prefix{Tools: tools, System: []string{"sys"}, Messages: messages, Breakpoints: []llm.Breakpoint{{Message: -1, TTL: "5m"}}},
	}
}

// Each line says what of the request before the request changed: this
// run's, and the last of the run before for the first of a run.
func TestPrefixLogSaysWhatChanged(t *testing.T) {
	path := filepath.Join(t.TempDir(), prefixLogFile)
	first := &prefixLog{path: path}
	first.report(prefixReport("t", "a", "b"))
	first.report(prefixReport("t", "a", "b", "c", "d"))
	first.report(prefixReport("t", "a", "x", "c", "d", "e"))
	first.report(prefixReport("t2", "a", "x", "c", "d", "e"))
	// The next run starts from a summary: its first message is new.
	next := &prefixLog{path: path}
	next.report(prefixReport("t2", "s", "f"))

	lines := prefixLines(t, path)
	want := []struct {
		changed string
		same    int
	}{{"first", 0}, {"", 2}, {"message 1", 1}, {"tools", 5}, {"message 0", 0}}
	if len(lines) != len(want) {
		t.Fatalf("%d lines", len(lines))
	}
	for index, line := range lines {
		if line.Changed != want[index].changed || line.Same != want[index].same {
			t.Errorf("line %d: changed %q, same %d; want %q, %d", index, line.Changed, line.Same, want[index].changed, want[index].same)
		}
	}
	if line := lines[1]; line.Model != "claude-opus-5" || line.Response != "msg_d" || line.Input != 1000 || line.Cached != 900 ||
		line.Written != 50 || len(line.Messages) != 4 || len(line.Breakpoints) != 1 || line.At.IsZero() {
		t.Fatalf("line = %+v", line)
	}
}

// The line a run was writing as it ended is not the last one; a log that
// is not the log is ignored; a log that cannot be written is let be.
func TestPrefixLogReadsTheLastWholeLine(t *testing.T) {
	path := filepath.Join(t.TempDir(), prefixLogFile)
	(&prefixLog{path: path}).report(prefixReport("t", "a", "b"))
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_APPEND, 0)
	if err != nil {
		t.Fatal(err)
	}
	file.WriteString(`{"at":"2026-10-09T00:00:00Z","messages":["a","zz`)
	file.Close()
	if previous := lastPrefix(path); previous == nil || len(previous.Messages) != 2 {
		t.Fatalf("previous = %+v", previous)
	}

	garbage := filepath.Join(t.TempDir(), prefixLogFile)
	os.WriteFile(garbage, []byte("not json\n"), 0o600)
	if lastPrefix(garbage) != nil || lastPrefix(filepath.Join(t.TempDir(), "none")) != nil {
		t.Fatal("a log that is not one gave a prefix")
	}

	unwritable := &prefixLog{path: filepath.Join(t.TempDir(), "missing", prefixLogFile)}
	unwritable.report(prefixReport("t", "a"))
	unwritable.report(prefixReport("t", "a", "b"))
	if !unwritable.broken {
		t.Fatal("the log that cannot be written was not let be")
	}
}
