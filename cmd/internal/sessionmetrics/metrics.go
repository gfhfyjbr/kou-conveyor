// Package sessionmetrics measures a session from its log: what its requests
// cost, how much of that went to rewriting the provider's cache and to
// requests that only wait, the messages it got, its compactions, heartbeats
// and stops. The numbers of the orchestration review come from it, and the
// checks of the changes it led to.
//
// Costs are in units of an uncached input token: a token read from the cache
// costs 0.1, one written to it 2 (the cache of an hour; 1.25 for five
// minutes), an output token 5.
package sessionmetrics

import (
	"bufio"
	"bytes"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

const (
	readWeight     = 0.1
	outputWeight   = 5
	hourWriteCost  = 2
	shortWriteCost = 1.25

	// A full rewrite writes more than rewriteTokens to the cache and reads
	// less than rewriteRead of its input from it: the cache missed most of
	// the conversation.
	rewriteTokens = 40000
	rewriteRead   = 0.3
	// A cold start reads less than coldRead from the cache, the system
	// prompt and the tools at most, and writes more than rewriteTokens.
	coldRead = 5000
	// A request whose input is pruneTokens or more below the one before
	// follows pruning, which cut old tool results.
	pruneTokens = 15000
	// The first windowRequests requests of an API turn whose turn two before
	// had thinking: the provider keeps the thinking of the latest two turns
	// that had it (clear_thinking), so the turn that drops out loses its
	// thinking there.
	windowRequests = 1
)

// coldKinds are the parts of ColdStarts, in their order.
var coldKinds = []string{"after compaction", "pause over TTL", "before compactions"}

// DefaultWait matches the shell commands that only wait.
var DefaultWait = regexp.MustCompile(`^\s*(sleep|kill)\b`)

// Options say what Read counts.
type Options struct {
	// Since leaves out what was recorded before it; zero counts it all.
	Since time.Time
	// TTL is how long the provider keeps the cache: a request after a longer
	// pause starts cold whatever it sends. Up to five minutes, a cache write
	// costs 1.25; longer, 2. Zero is an hour.
	TTL time.Duration
	// Wait matches the shell commands (the command of a Bash call) that only
	// wait; nil is DefaultWait. Waits of the canvas (CanvasWait, CanvasRead
	// with wait) are waits whatever it matches.
	Wait *regexp.Regexp
}

// Report is what Read measured.
type Report struct {
	Session string    `json:"session,omitzero"`
	Since   time.Time `json:"since,omitzero"`
	// First and Last are when the first and the last item counted were
	// recorded.
	First time.Time `json:"first,omitzero"`
	Last  time.Time `json:"last,omitzero"`
	// Turns counts the turns the session recorded: each is one request,
	// unless it failed. APITurns counts the requests that began a turn of
	// the conversation the provider sees: after a prompt, a heartbeat, a
	// result that came after the response that waited for it, or a response
	// without tool calls.
	Turns    int `json:"turns"`
	Requests int `json:"requests"`
	APITurns int `json:"api_turns"`
	// Cost is the cost of the requests, in units; Tokens the tokens they
	// sent and got.
	Cost   float64 `json:"cost"`
	Tokens Tokens  `json:"tokens"`
	// Rewrites are the requests that wrote most of the conversation to the
	// cache anew, with what caused it.
	Rewrites Rewrites `json:"rewrites"`
	// ColdStarts are the requests that found nothing in the cache past the
	// system prompt and the tools.
	ColdStarts ColdStarts `json:"cold_starts"`
	// Waits are the requests that only waited: their every tool call waits.
	Waits       Waits       `json:"waits"`
	Messages    Messages    `json:"messages"`
	Compactions Compactions `json:"compactions"`
	// Heartbeats counts the harness's messages to the model by kind:
	// "progress report", "compaction soon", "after compaction", "notes
	// changed", "stopped before", "waited for tools", "output limit",
	// "refused", "other".
	Heartbeats map[string]int `json:"heartbeats,omitzero"`
	// Stops are the stops the session recorded, with their reasons.
	Stops      []Stop     `json:"stops,omitzero"`
	ToolErrors ToolErrors `json:"tool_errors"`
	// Unreadable counts the lines Read could not decode, Problem the first
	// such problem: a log written by a newer harness may have them.
	Unreadable int    `json:"unreadable,omitzero"`
	Problem    string `json:"problem,omitzero"`
}

// Tokens are the tokens of requests.
type Tokens struct {
	Input   int64 `json:"input"`
	Read    int64 `json:"read"`
	Written int64 `json:"written"`
	Output  int64 `json:"output"`
}

// Rewrites are the full rewrites of the cache. The cost of one is that of
// what it wrote; each counts under the first of its causes, in the order
// "new run", "thinking window", "prune", "other". Also counts each cause
// with the rewrites it came with, whatever came first.
type Rewrites struct {
	Count  int             `json:"count"`
	Cost   float64         `json:"cost"`
	Share  float64         `json:"share"`
	Causes []Part          `json:"causes,omitzero"`
	Also   map[string]int  `json:"also,omitzero"`
	List   []RewriteDetail `json:"list,omitzero"`
}

// Part is one part of a count, with its cost and its share of all the cost.
type Part struct {
	Name  string  `json:"name"`
	Count int     `json:"count"`
	Cost  float64 `json:"cost"`
	Share float64 `json:"share"`
}

// RewriteDetail is one request that rewrote the cache.
type RewriteDetail struct {
	At      time.Time `json:"at"`
	Input   int64     `json:"input"`
	Read    int64     `json:"read"`
	Written int64     `json:"written"`
	// Causes are what came with it: "new run" (the cache held nothing past
	// the system prompt), "thinking window" (the first request of an API
	// turn whose turn two before had thinking), "prune" (the input shrank).
	// A cold start has its part instead.
	Causes []string `json:"causes,omitzero"`
	// Pause is the time since the request before.
	Pause Minutes `json:"pause_minutes"`
	// Compacted says whether a compaction came before it in the session.
	Compacted bool `json:"compacted,omitzero"`
}

// ColdStarts are the requests that started from an empty cache, as parts:
// "after compaction" (a compaction came before and the pause was within the
// TTL: what the compaction left at the start of the conversation was made
// anew), "pause over TTL" (after a compaction too, but the cache had
// expired), "before compactions" (none came before, whatever the pause).
type ColdStarts struct {
	Count int             `json:"count"`
	Cost  float64         `json:"cost"`
	Share float64         `json:"share"`
	Parts []Part          `json:"parts,omitzero"`
	List  []RewriteDetail `json:"list,omitzero"`
}

// Waits are the requests whose tool calls all only waited.
type Waits struct {
	Requests int     `json:"requests"`
	Cost     float64 `json:"cost"`
	Share    float64 `json:"share"`
	// Calls counts the tool calls that waited, in any request.
	Calls int `json:"calls"`
}

// Messages are the prompts the session got. The log records one when it
// reaches the model: how long it waited before, in the cockpit's queue or
// for the tool calls of the moment, is not in it.
type Messages struct {
	Count int `json:"count"`
	// AfterTools came while tool calls ran, and went to the model with
	// their results.
	AfterTools int `json:"after_tools"`
	// OpenedRun began a run: the session was idle, or the message had
	// waited in the queue for it.
	OpenedRun int `json:"opened_run"`
	// FromCanvas came from other nodes of a canvas, Replies of them as
	// answers to what the session sent.
	FromCanvas int `json:"from_canvas"`
	Replies    int `json:"replies"`
}

// Compactions are the compactions of the session.
type Compactions struct {
	Count int `json:"count"`
	// Requests counts their requests: a compaction tried again has more.
	Requests int `json:"requests"`
	// Kept counts the compactions whose message the log keeps (it does since
	// the harness keeps it), NotesClipped those of them whose message showed
	// only the beginning and end of the notes, and OverflowAsks the times
	// the harness asked the model to write its notes shorter.
	Kept         int `json:"kept"`
	NotesClipped int `json:"notes_clipped"`
	OverflowAsks int `json:"overflow_asks"`
}

// Stop is a stop the session recorded.
type Stop struct {
	At     time.Time `json:"at"`
	Reason string    `json:"reason,omitzero"`
}

// ToolErrors are the tool calls that failed before they ran, Arguments
// those whose arguments could not be decoded.
type ToolErrors struct {
	Count     int `json:"count"`
	Arguments int `json:"arguments"`
}

// Minutes is a duration in minutes, as JSON.
type Minutes float64

// Read measures the session log r.
func Read(r io.Reader, options Options) (Report, error) {
	m := newMeter(options)
	in := bufio.NewReaderSize(r, 1<<20)
	for {
		line, err := in.ReadBytes('\n')
		if len(line) > 0 && (err == nil || errors.Is(err, io.EOF)) {
			if err == nil {
				m.line(line)
			} else if len(bytes.TrimSpace(line)) > 0 {
				// A line without its newline is being written.
				m.unreadable(fmt.Errorf("the last line is not complete"))
			}
		}
		if errors.Is(err, io.EOF) {
			break
		}
		if err != nil {
			return Report{}, err
		}
	}
	return m.finish(), nil
}

// meter folds a session log into a Report.
type meter struct {
	since time.Time
	ttl   time.Duration
	wait  *regexp.Regexp
	write float64

	report Report
	turns  map[session.TurnID]session.TurnType
	// calls maps operations to the tool calls that wait for them; ended
	// holds the operations that ended.
	calls map[operation.ID]string
	ended map[operation.ID]bool

	// Since the request before: the operations that ended, and whether a
	// prompt or a heartbeat came.
	finished []operation.ID
	prompted bool

	previous    *request
	compactions int // compaction requests so far
	// thinking says, for each API turn since the latest compaction, whether
	// it had thinking; position is the request's place in its turn.
	thinking []bool
	position int
	// settings says the latest item was a settings input, which opens the
	// run of a prompt sent from a cockpit.
	settings bool

	rewrites, colds map[string]*Part
}

type request struct {
	at         time.Time
	usage      llm.Usage
	compaction bool
	calls      []string
	thinking   bool
}

func newMeter(options Options) *meter {
	m := &meter{
		since: options.Since.UTC(),
		ttl:   options.TTL,
		wait:  options.Wait,
		write: hourWriteCost,
		turns: map[session.TurnID]session.TurnType{},
		calls: map[operation.ID]string{},
		ended: map[operation.ID]bool{},
		rewrites: map[string]*Part{
			"new run": {Name: "new run"}, "thinking window": {Name: "thinking window"},
			"prune": {Name: "prune"}, "other": {Name: "other"},
		},
		colds: map[string]*Part{
			"after compaction": {Name: "after compaction"}, "pause over TTL": {Name: "pause over TTL"},
			"before compactions": {Name: "before compactions"},
		},
	}
	if m.ttl <= 0 {
		m.ttl = time.Hour
	}
	if m.ttl <= 5*time.Minute {
		m.write = shortWriteCost
	}
	if m.wait == nil {
		m.wait = DefaultWait
	}
	m.report.Since = options.Since
	return m
}

func (m *meter) unreadable(err error) {
	if m.report.Unreadable == 0 {
		m.report.Problem = err.Error()
	}
	m.report.Unreadable++
}

func (m *meter) line(line []byte) {
	line = bytes.TrimSpace(line)
	if len(line) == 0 {
		return
	}
	var envelope struct {
		Type string         `json:"type"`
		Data jsontext.Value `json:"data"`
	}
	if err := json.Unmarshal(line, &envelope); err != nil {
		m.unreadable(fmt.Errorf("decode a record: %w", err))
		return
	}
	switch envelope.Type {
	case "session":
		var record struct{ Session session.Session }
		if err := json.Unmarshal(envelope.Data, &record); err != nil {
			m.unreadable(fmt.Errorf("decode the session record: %w", err))
			return
		}
		m.report.Session = string(record.Session.ID)
	case "operation":
		var record struct{ Operation operation.Operation }
		if err := json.Unmarshal(envelope.Data, &record); err != nil {
			m.unreadable(fmt.Errorf("decode an operation record: %w", err))
			return
		}
		m.operation(record.Operation)
	case "item":
		var record struct {
			Item       sessionstore.Item
			Operations []operation.Operation
		}
		if err := json.Unmarshal(envelope.Data, &record); err != nil {
			m.unreadable(fmt.Errorf("decode an item record: %w", err))
			return
		}
		for _, value := range record.Operations {
			m.operation(value)
		}
		m.item(record.Item)
	default:
		m.unreadable(fmt.Errorf("unknown record type %q", envelope.Type))
	}
}

func (m *meter) operation(value operation.Operation) {
	switch value.Status {
	case operation.StatusCompleted, operation.StatusFailed, operation.StatusCanceled:
	default:
		return
	}
	if m.ended[value.ID] {
		return
	}
	m.ended[value.ID] = true
	m.finished = append(m.finished, value.ID)
}

// counted says whether what was recorded at at is counted.
func (m *meter) counted(at time.Time) bool {
	if at.Before(m.since) {
		return false
	}
	if m.report.First.IsZero() || at.Before(m.report.First) {
		m.report.First = at
	}
	if at.After(m.report.Last) {
		m.report.Last = at
	}
	return true
}

func (m *meter) item(item sessionstore.Item) {
	settings := false
	switch data := item.Data.(type) {
	case session.Turn:
		previous, known := m.turns[data.PreviousTurnID]
		m.turns[data.ID] = data.Type
		if !m.counted(item.RecordedAt) {
			break
		}
		m.report.Turns++
		if data.Type == session.TurnCompaction && (!known || previous != session.TurnCompaction) {
			m.report.Compactions.Count++
		}
	case inbox.Input:
		settings = m.input(item.RecordedAt, data)
	case sessionstore.ToolCallStatus:
		for _, id := range data.Status.WaitingFor {
			m.calls[id] = data.CallID
		}
		if data.Status.Error != "" && m.counted(item.RecordedAt) {
			m.report.ToolErrors.Count++
			if strings.HasPrefix(data.Status.Error, "decode ") && strings.Contains(data.Status.Error, "arguments") {
				m.report.ToolErrors.Arguments++
			}
		}
	case sessionstore.ModelResponse:
		m.response(item.RecordedAt, data)
	}
	m.settings = settings
}

// input counts an input and says whether it was a settings input.
func (m *meter) input(at time.Time, input inbox.Input) bool {
	if input.Kind != inbox.InputControl {
		m.prompted = true
		if input.Kind != inbox.InputExternal || !m.counted(at) {
			return false
		}
		messages := &m.report.Messages
		messages.Count++
		if input.Delivery == inbox.DeliverAfterTools {
			messages.AfterTools++
		}
		if m.settings {
			messages.OpenedRun++
		}
		text := promptText(input.Payload)
		switch {
		case strings.HasPrefix(text, "[canvas] reply from "):
			messages.FromCanvas++
			messages.Replies++
		case strings.HasPrefix(text, "[canvas] from "):
			messages.FromCanvas++
		}
		return false
	}
	// Control inputs are read leniently: one a newer harness wrote may have
	// what this one does not know.
	var control struct {
		Mode   inbox.ControlMode
		Reason string
	}
	if json.Unmarshal(input.Payload, &control) != nil {
		return false
	}
	switch control.Mode {
	case inbox.Heartbeat:
		m.prompted = true
		if !m.counted(at) {
			break
		}
		if m.report.Heartbeats == nil {
			m.report.Heartbeats = map[string]int{}
		}
		m.report.Heartbeats[heartbeatKind(control.Reason)]++
		if strings.Contains(control.Reason, overflowAsk) {
			m.report.Compactions.OverflowAsks++
		}
	case inbox.StopHard:
		if m.counted(at) {
			m.report.Stops = append(m.report.Stops, Stop{At: at, Reason: control.Reason})
		}
	case inbox.UpdateSettings:
		return true
	}
	return false
}

// promptText is the text of a prompt's payload: a JSON string, or a message
// with images.
func promptText(payload jsontext.Value) string {
	var text string
	if json.Unmarshal(payload, &text) == nil {
		return text
	}
	var message struct {
		Text string
	}
	json.Unmarshal(payload, &message)
	return message.Text
}

// overflowAsk is in the harness's request to write the notes shorter
// (agentrunner's notesOverflow).
const overflowAsk = "Rewrite them shorter now"

// heartbeatKinds tell the harness's messages apart by what they say, the
// first that matches.
var heartbeatKinds = []struct{ kind, text string }{
	{"progress report", "Progress report"},
	{"compaction soon", "will be compacted soon"},
	{"stopped before", "The previous run was stopped"},
	{"notes changed", "Your notes changed"},
	{"after compaction", "at this compaction is snapshot"},
	{"after compaction", "compactions in a row came with no file changed"},
	{"after compaction", overflowAsk},
	{"waited for tools", "Heartbeat: waited"},
	{"output limit", "cut off at the output token limit"},
	{"refused", "previous response was refused"},
}

func heartbeatKind(reason string) string {
	for _, k := range heartbeatKinds {
		if strings.Contains(reason, k.text) {
			return k.kind
		}
	}
	return "other"
}

func (m *meter) waits(call llm.ToolCall) bool {
	switch {
	case strings.HasSuffix(call.Name, "Bash"):
		var arguments struct {
			Command string `json:"command"`
		}
		json.Unmarshal([]byte(call.Arguments), &arguments)
		return m.wait.MatchString(arguments.Command)
	case strings.HasSuffix(call.Name, "CanvasWait"):
		return true
	case strings.HasSuffix(call.Name, "CanvasRead"):
		var arguments struct {
			Wait string `json:"wait"`
		}
		json.Unmarshal([]byte(call.Arguments), &arguments)
		return arguments.Wait != ""
	}
	return false
}

func (m *meter) cost(usage llm.Usage) float64 {
	uncached := max(usage.InputTokens-usage.CachedInputTokens-usage.CacheWriteInputTokens, 0)
	return float64(usage.CachedInputTokens)*readWeight + float64(usage.CacheWriteInputTokens)*m.write +
		float64(uncached) + float64(usage.OutputTokens)*outputWeight
}

func (m *meter) response(at time.Time, data sessionstore.ModelResponse) {
	usage := data.Response.Usage
	if m.turns[data.TurnID] == session.TurnCompaction && data.Compacted != nil && m.counted(at) {
		m.report.Compactions.Kept++
		if data.Compacted.Notes != nil && strings.Contains(*data.Compacted.Notes, " characters left out …]") {
			m.report.Compactions.NotesClipped++
		}
	}
	if usage.InputTokens == 0 {
		return
	}
	current := &request{at: at, usage: usage, compaction: m.turns[data.TurnID] == session.TurnCompaction}
	waits := 0
	for _, output := range data.Response.Output {
		switch value := output.Data.(type) {
		case llm.ToolCall:
			current.calls = append(current.calls, value.CallID)
			if m.waits(value) {
				waits++
			}
		case llm.Reasoning:
			current.thinking = true
		}
	}
	previous := m.previous

	// A request begins an API turn after a prompt or a heartbeat, after a
	// result that came after the response that waited for it, or after a
	// response without tool calls; a compaction is a conversation of its
	// own, and the one after it begins anew.
	newTurn := true
	if !current.compaction && previous != nil && !previous.compaction {
		late := false
		for _, id := range m.finished {
			if call, ok := m.calls[id]; ok && !slices.Contains(previous.calls, call) {
				late = true
			}
		}
		newTurn = late || m.prompted || len(previous.calls) == 0
	}
	window := false
	if current.compaction {
		m.thinking, m.position = nil, 0
	} else {
		if newTurn || len(m.thinking) == 0 {
			m.thinking = append(m.thinking, false)
			m.position = 1
		} else {
			m.position++
		}
		turn := len(m.thinking) - 1
		window = m.position <= windowRequests && turn >= 2 && m.thinking[turn-2]
		if current.thinking {
			m.thinking[turn] = true
		}
	}

	if m.counted(at) {
		cost := m.cost(usage)
		m.report.Requests++
		m.report.Cost += cost
		m.report.Tokens.Input += usage.InputTokens
		m.report.Tokens.Read += usage.CachedInputTokens
		m.report.Tokens.Written += usage.CacheWriteInputTokens
		m.report.Tokens.Output += usage.OutputTokens
		if current.compaction {
			m.report.Compactions.Requests++
		} else if newTurn && previous != nil {
			m.report.APITurns++
		}
		m.report.Waits.Calls += waits
		if waits > 0 && waits == len(current.calls) {
			m.report.Waits.Requests++
			m.report.Waits.Cost += cost
		}
		written := float64(usage.CacheWriteInputTokens) * m.write
		detail := RewriteDetail{
			At: at, Input: usage.InputTokens, Read: usage.CachedInputTokens, Written: usage.CacheWriteInputTokens,
			Compacted: m.compactions > 0,
		}
		if previous != nil {
			detail.Pause = Minutes(at.Sub(previous.at).Minutes())
		}
		cold := usage.CachedInputTokens < coldRead && usage.CacheWriteInputTokens > rewriteTokens
		if previous != nil && !previous.compaction && !current.compaction &&
			usage.CacheWriteInputTokens > rewriteTokens && float64(usage.CachedInputTokens) < rewriteRead*float64(usage.InputTokens) {
			if cold {
				detail.Causes = append(detail.Causes, "new run")
			}
			if window {
				detail.Causes = append(detail.Causes, "thinking window")
			}
			if previous.usage.InputTokens-usage.InputTokens >= pruneTokens {
				detail.Causes = append(detail.Causes, "prune")
			}
			cause := "other"
			if len(detail.Causes) > 0 {
				cause = detail.Causes[0]
			}
			part := m.rewrites[cause]
			part.Count++
			part.Cost += written
			for _, also := range detail.Causes {
				if m.report.Rewrites.Also == nil {
					m.report.Rewrites.Also = map[string]int{}
				}
				m.report.Rewrites.Also[also]++
			}
			m.report.Rewrites.Count++
			m.report.Rewrites.Cost += written
			m.report.Rewrites.List = append(m.report.Rewrites.List, detail)
		}
		if cold {
			kind := "after compaction"
			switch {
			case m.compactions == 0:
				kind = "before compactions"
			case previous != nil && at.Sub(previous.at) > m.ttl:
				kind = "pause over TTL"
			}
			detail.Causes = []string{kind}
			part := m.colds[kind]
			part.Count++
			part.Cost += written
			m.report.ColdStarts.Count++
			m.report.ColdStarts.Cost += written
			m.report.ColdStarts.List = append(m.report.ColdStarts.List, detail)
		}
	}
	if current.compaction {
		m.compactions++
	}
	m.previous = current
	m.finished = m.finished[:0]
	m.prompted = false
}

func (m *meter) finish() Report {
	r := m.report
	share := func(cost float64) float64 {
		if r.Cost == 0 {
			return 0
		}
		return cost / r.Cost
	}
	r.Rewrites.Share = share(r.Rewrites.Cost)
	for _, name := range []string{"new run", "thinking window", "prune", "other"} {
		part := *m.rewrites[name]
		part.Share = share(part.Cost)
		r.Rewrites.Causes = append(r.Rewrites.Causes, part)
	}
	r.ColdStarts.Share = share(r.ColdStarts.Cost)
	for _, name := range coldKinds {
		part := *m.colds[name]
		part.Share = share(part.Cost)
		r.ColdStarts.Parts = append(r.ColdStarts.Parts, part)
	}
	r.Waits.Share = share(r.Waits.Cost)
	return r
}
