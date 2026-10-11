package agentrunner

import (
	"bytes"
	"encoding/json/v2"
	"fmt"
	"io"
	"os"
	"slices"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// prefixLogFile is kept in the session's operations folder, which goes
// with the session when it is deleted.
const prefixLogFile = "prefix.jsonl"

// prefixLog keeps prefix.jsonl: a line for each request to the model a
// response answered, with the fingerprints of what it began with
// (llm.Prefix), the tokens the provider read from its cache and wrote to
// it, and the first part of the request before it — this run's, or the
// last of the run before — that it changed. Where a request stopped being
// the one before is where the cache stopped serving it: the log says why a
// request cost what it did without the requests, which are megabytes.
//
// The log is a diagnosis: a run whose log cannot be written goes on
// without it.
type prefixLog struct {
	path     string
	mu       sync.Mutex
	loaded   bool
	previous *llm.Prefix
	broken   bool
}

// prefixLine is a line of prefix.jsonl.
type prefixLine struct {
	At       time.Time `json:"at"`
	Model    string    `json:"model,omitempty"`
	Response string    `json:"response,omitempty"`
	// Input is all the request's tokens; Cached those read from the cache,
	// Written those written to it.
	Input   int64 `json:"input"`
	Cached  int64 `json:"cached"`
	Written int64 `json:"written"`
	// Changed is the first part of the request before that this one does
	// not begin with: "tools", "system N", "edits" or "message N"; empty
	// when it begins with all of it, and "first" when no request came
	// before.
	Changed string `json:"changed,omitempty"`
	// Same is how many messages the request begins with that the request
	// before began with too.
	Same int `json:"same"`
	llm.Prefix
}

// report appends the line of a request.
func (l *prefixLog) report(report llm.PrefixReport) {
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.broken || len(report.Prefix.Messages) == 0 {
		return
	}
	if !l.loaded {
		l.previous, l.loaded = lastPrefix(l.path), true
	}
	line := prefixLine{
		At: time.Now().UTC(), Model: report.Model, Response: report.Response,
		Input: report.Usage.InputTokens, Cached: report.Usage.CachedInputTokens, Written: report.Usage.CacheWriteInputTokens,
		Changed: "first", Prefix: report.Prefix,
	}
	if l.previous != nil {
		line.Changed, line.Same = prefixChange(*l.previous, report.Prefix)
	}
	data, err := json.Marshal(line)
	if err == nil {
		err = appendLine(l.path, data)
	}
	if err != nil {
		l.broken = true
		return
	}
	l.previous = &report.Prefix
}

// prefixChange says what of the request before now does not begin with
// (prefixLine.Changed), and how many messages they begin with alike.
func prefixChange(before, now llm.Prefix) (string, int) {
	same := 0
	for same < len(before.Messages) && same < len(now.Messages) && before.Messages[same] == now.Messages[same] {
		same++
	}
	switch {
	case before.Tools != now.Tools:
		return "tools", same
	case !slices.Equal(before.System, now.System):
		index := 0
		for index < len(before.System) && index < len(now.System) && before.System[index] == now.System[index] {
			index++
		}
		return fmt.Sprintf("system %d", index), same
	case !bytes.Equal(before.Edits, now.Edits):
		return "edits", same
	case same < len(before.Messages):
		return fmt.Sprintf("message %d", same), same
	}
	return "", same
}

func appendLine(path string, data []byte) error {
	file, err := os.OpenFile(path, os.O_WRONLY|os.O_APPEND|os.O_CREATE, 0o600)
	if err != nil {
		return err
	}
	_, err = file.Write(append(data, '\n'))
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	return err
}

// lastPrefixRead is how much of the end of the log is read for its last
// line: a line holds 11 bytes a message.
const lastPrefixRead = 4 << 20

// lastPrefix is the prefix of the log's last whole line, or nil.
func lastPrefix(path string) *llm.Prefix {
	file, err := os.Open(path)
	if err != nil {
		return nil
	}
	defer file.Close()
	info, err := file.Stat()
	if err != nil {
		return nil
	}
	offset := max(info.Size()-lastPrefixRead, 0)
	data, err := io.ReadAll(io.NewSectionReader(file, offset, info.Size()-offset))
	if err != nil {
		return nil
	}
	// A line a run was writing as it ended has no newline: the one before
	// it is the last whole one.
	end := bytes.LastIndexByte(data, '\n')
	if end < 0 {
		return nil
	}
	start := bytes.LastIndexByte(data[:end], '\n') + 1
	if start == 0 && offset > 0 {
		return nil
	}
	var line prefixLine
	if json.Unmarshal(data[start:end], &line) != nil {
		return nil
	}
	return &line.Prefix
}
