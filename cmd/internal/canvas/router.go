package canvas

import (
	"encoding/json/jsontext"
	"errors"
	"fmt"
	"time"
	"unicode/utf8"
)

// Routing. A node gives an output on a port (emit); every edge from that
// port that is not off makes it a message for the node at its other end:
// the edge's template makes its text, under a line that says where it comes
// from when the target is an agent. Messages wait in the canvas's queue,
// kept on disk, until their target can take them — idle, for the default
// delivery, or at once for "now" — or, on an edge that asks for approval,
// until the user approves them. One goroutine delivers them, in the order
// they came.
//
// What a node gives after a message reached it goes on that message's
// chain, one hop further: a chain longer than the canvas's max_hops is a
// loop, and its message is dropped. Edges and the canvas deliver at most so
// many messages a minute, the others wait; an edge whose deliveries fail
// three times in a row turns itself off.

// States of a message.
const (
	MessagePending   = "pending"
	MessageAwaiting  = "awaiting_approval"
	MessageDelivered = "delivered"
	MessageDropped   = "dropped"
	// messageDelivering is a message on its way, for the dispatcher only.
	messageDelivering = "delivering"
)

const (
	// maxMessage is the longest text delivered whole; a longer one is
	// kept in a file, and its first deliveredHead bytes go with the file's
	// path.
	maxMessage    = 32 << 10
	deliveredHead = 8 << 10
	// previewLength bounds the text the pages are given of a message.
	previewLength = 300
	// breakerFailures is how many failed deliveries in a row turn an
	// edge off.
	breakerFailures = 3
)

// Message is what goes along an edge, or straight to a node.
type Message struct {
	ID string `json:"id"`
	// Chain is the first message of those that caused this one; Hops
	// counts them.
	Chain string    `json:"chain"`
	Hops  int       `json:"hops"`
	From  Port      `json:"from"`
	Edge  string    `json:"edge,omitzero"`
	To    Port      `json:"to"`
	At    time.Time `json:"at"`
	Title string    `json:"title,omitzero"`
	Text  string    `json:"text"`
	// Data is what the output gave besides its text.
	Data jsontext.Value `json:"data,omitzero"`
	// File keeps the whole text of a message too long to deliver whole.
	File    string `json:"file,omitzero"`
	State   string `json:"state"`
	Reason  string `json:"reason,omitzero"`
	Deliver string `json:"deliver,omitzero"`
	// NoSubmit types a message into a terminal without pressing Enter.
	NoSubmit bool `json:"no_submit,omitzero"`
	// Actor sent it straight to its node: user, or a node's ID.
	Actor string `json:"actor,omitzero"`
	// Reply marks the answer to a message a node sent another straight
	// (InReplyTo), which comes back to it: what it answers to a reply
	// stays with it.
	Reply       bool      `json:"reply,omitzero"`
	InReplyTo   string    `json:"in_reply_to,omitzero"`
	DeliveredAt time.Time `json:"delivered_at,omitzero"`
	// Truncated says that Text is only the start of the message's text:
	// the pages are given that much.
	Truncated bool `json:"truncated,omitzero"`
}

// preview is a message as the pages see it: the start of its text.
func (m *Message) preview() *Message {
	copied := *m
	copied.Data = nil
	if len(copied.Text) > previewLength {
		copied.Text, copied.Truncated = cut(copied.Text, previewLength)+"…", true
	}
	return &copied
}

// cut is the start of s, at most n bytes, whole runes.
func cut(s string, n int) string {
	if len(s) <= n {
		return s
	}
	for n > 0 && !utf8.RuneStart(s[n]) {
		n--
	}
	return s[:n]
}

// Output is what a node gives on one of its ports.
type Output struct {
	Port  string
	Title string
	Text  string
	Data  jsontext.Value
}

// emit has a node give an output.
func (c *Canvas) emit(node string, out Output) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.emitLocked(node, out)
}

// emitLocked has a node give an output on the chain of the message
// delivered to it last.
func (c *Canvas) emitLocked(node string, out Output) {
	st := c.states[node]
	if st == nil {
		return
	}
	cause := st.cause
	st.cause = nil
	c.putOutLocked(node, out, cause, nil)
}

// putOutLocked has a node give an output, one hop further on cause's
// chain (a chain of its own without one): along its edges, except those to
// the nodes of skip, which have it already.
func (c *Canvas) putOutLocked(node string, out Output, cause *Message, skip map[string]bool) {
	n, st := c.doc.node(node), c.states[node]
	if n == nil || st == nil || c.deleted {
		return
	}
	if out.Port == "" {
		out.Port = "out"
	}
	st.output = out.Text
	st.outputs++
	st.wake()
	c.hub.publish(map[string]any{"type": "output", "node": node, "port": out.Port, "title": out.Title, "preview": previewOf(out.Text), "at": now()})
	chain, hops := newID("c_", 10), 0
	if cause != nil {
		chain, hops = cause.Chain, cause.Hops+1
	}
	for _, e := range c.doc.Edges {
		if e.From.Node != node || e.From.Port != out.Port || e.mode() == ModeOff || skip[e.To.Node] {
			continue
		}
		target := c.doc.node(e.To.Node)
		if target == nil {
			continue
		}
		header := c.answersLocked(target)
		if e.Header != nil {
			header = *e.Header
		}
		vars := messageVars{text: out.Text, title: out.Title, data: out.Data, from: n, edge: e.ID, now: time.Now()}
		m := &Message{
			ID: newID("m_", 10), Chain: chain, Hops: hops, From: Port{node, out.Port}, Edge: e.ID, To: e.To, At: now(),
			Title: out.Title, Text: renderMessage(e.Template, vars, header), Data: out.Data, Deliver: e.deliver(),
		}
		if limit := c.doc.Settings.Routing.MaxHops; hops > limit {
			m.State, m.Reason = MessageDropped, fmt.Sprintf("its chain went over %d hops: a loop?", limit)
			c.recordLocked(m)
			c.noticeLocked("warning", fmt.Sprintf("A message from «%s» to «%s» was dropped: its chain went over %d hops, which looks like a loop.", n.Title, target.Title, limit), "", e.ID)
			continue
		}
		c.longLocked(m)
		m.State = MessagePending
		if e.mode() == ModeApprove {
			m.State = MessageAwaiting
		}
		c.enqueueLocked(m)
	}
}

func previewOf(text string) string {
	if len(text) > previewLength {
		return cut(text, previewLength) + "…"
	}
	return text
}

// longLocked keeps the whole text of a message too long to deliver whole
// in a file, and has its start delivered with the file's path.
func (c *Canvas) longLocked(m *Message) {
	if len(m.Text) <= maxMessage {
		return
	}
	path, err := saveLong(c.dir, m.ID, m.Text)
	if err != nil {
		m.Text = cut(m.Text, maxMessage) + "\n…(cut: the message was too long)"
		return
	}
	m.File = path
	m.Text = cut(m.Text, deliveredHead) + "\n…\nFull text: " + path
}

// enqueueLocked has a message wait for its delivery or approval.
func (c *Canvas) enqueueLocked(m *Message) {
	c.pending = append(c.pending, m)
	c.pendingDirty = true
	c.scheduleSaveLocked()
	c.recordLocked(m)
	c.kickDispatch()
}

// recordLocked keeps a message as it is now among the canvas's latest, and
// tells the pages; one that came to its end goes to the journal.
func (c *Canvas) recordLocked(m *Message) {
	replaced := false
	for i := len(c.messages) - 1; i >= 0 && i >= len(c.messages)-recentMessages; i-- {
		if c.messages[i].ID == m.ID {
			c.messages[i] = m
			replaced = true
			break
		}
	}
	if !replaced {
		c.messages = append(c.messages, m)
		if len(c.messages) > recentMessages {
			c.messages = append(c.messages[:0:0], c.messages[len(c.messages)-recentMessages:]...)
		}
	}
	if m.State == MessageDelivered || m.State == MessageDropped {
		copied := *m
		c.journal = append(c.journal, &copied)
		c.scheduleSaveLocked()
	}
	c.hub.publish(map[string]any{"type": "message", "message": m.preview()})
}

// unqueueLocked takes a message out of those that wait; what waits for its
// node to be idle looks again.
func (c *Canvas) unqueueLocked(m *Message) {
	for i, other := range c.pending {
		if other == m {
			c.pending = append(c.pending[:i:i], c.pending[i+1:]...)
			c.pendingDirty = true
			c.scheduleSaveLocked()
			if st := c.states[m.To.Node]; st != nil {
				st.wake()
			}
			return
		}
	}
}

// pendingLocked finds a message that waits.
func (c *Canvas) pendingLocked(id string) *Message {
	for _, m := range c.pending {
		if m.ID == id {
			return m
		}
	}
	return nil
}

// dropPendingLocked drops the messages that wait for or come from a node.
func (c *Canvas) dropPendingLocked(node, reason string) {
	kept := c.pending[:0]
	for _, m := range c.pending {
		// One being delivered is the dispatcher's until it is done.
		if m.State == messageDelivering {
			kept = append(kept, m)
			continue
		}
		if m.To.Node == node || m.From.Node == node && m.State == MessageAwaiting {
			m.State, m.Reason = MessageDropped, reason
			c.recordLocked(m)
			c.settleLocked(m.ID, "", "it was dropped: "+reason)
			c.pendingDirty = true
			continue
		}
		kept = append(kept, m)
	}
	c.pending = kept
	c.scheduleSaveLocked()
}

// kickDispatch has the dispatcher look at the messages that wait.
func (c *Canvas) kickDispatch() {
	select {
	case c.kick <- struct{}{}:
	default:
	}
}

// dispatch delivers the messages that wait, as their targets can take
// them, until the canvas is closed.
func (c *Canvas) dispatch() {
	ticker := time.NewTicker(time.Second)
	defer ticker.Stop()
	for {
		select {
		case <-c.kick:
		case <-ticker.C:
		case <-c.done:
			return
		}
		for c.dispatchOne() {
		}
	}
}

// readyLocked reports whether a node takes a message now.
func (c *Canvas) readyLocked(n *Node, st *nodeState, deliver string) bool {
	if n == nil || st == nil || st.inst == nil || n.Proposed {
		return false
	}
	switch n.Kind {
	case KindAgent:
		return true
	case KindTerminal:
		switch st.status.State {
		case StateExited, StateStopped, StateStarting, StateError, StatePaused:
			return false
		}
		return deliver == DeliverNow || st.status.State == StateIdle
	}
	return false
}

// dispatchOne delivers the first message that can go, and reports whether
// there was one.
func (c *Canvas) dispatchOne() bool {
	c.mu.Lock()
	if c.closed || c.deleted {
		c.mu.Unlock()
		return false
	}
	t := time.Now()
	blocked := map[string]bool{}
	for _, m := range c.pending {
		target := m.To.Node
		if m.State != MessagePending || blocked[target] {
			continue
		}
		// A paused canvas keeps what its nodes give; what the user sends
		// goes all the same.
		if !c.doc.Live && m.Actor != "user" {
			continue
		}
		n, st := c.doc.node(target), c.states[target]
		if n == nil {
			m.State, m.Reason = MessageDropped, "its node is gone"
			c.unqueueLocked(m)
			c.recordLocked(m)
			c.settleLocked(m.ID, "", "its node is gone")
			c.mu.Unlock()
			return true
		}
		if !c.readyLocked(n, st, m.Deliver) {
			blocked[target] = true
			continue
		}
		if reason := c.rateLocked(m, t); reason != "" {
			if m.Reason != reason {
				m.Reason = reason
				c.recordLocked(m)
			}
			blocked[target] = true
			continue
		}
		m.State, m.Reason = messageDelivering, ""
		inst := st.inst
		// What the node gives from now on is this message's doing, and a
		// terminal is busy with it: both are said before the text goes in,
		// as a quick command ends before its delivery returns.
		before := st.status
		st.cause = m
		if ti, ok := inst.(*terminalInstance); ok {
			ti.markDelivered()
			c.setStatusLocked(target, StateBusy, "delivered")
		}
		c.mu.Unlock()
		err := inst.deliver(m)
		c.mu.Lock()
		c.delivered(m, n, err, before)
		c.mu.Unlock()
		return true
	}
	c.mu.Unlock()
	return false
}

// rateLocked says why a message must wait for the limits of its edge and
// of the canvas, if it must.
func (c *Canvas) rateLocked(m *Message, t time.Time) string {
	routing := c.doc.Settings.Routing
	recent := func(key string) int {
		kept := c.rates[key][:0]
		for _, at := range c.rates[key] {
			if t.Sub(at) < time.Minute {
				kept = append(kept, at)
			}
		}
		c.rates[key] = kept
		return len(kept)
	}
	if m.Edge != "" && recent(m.Edge) >= routing.EdgeRatePerMinute {
		return fmt.Sprintf("rate-limited: the edge delivers %d messages a minute at most", routing.EdgeRatePerMinute)
	}
	if recent("") >= routing.CanvasRatePerMinute {
		return fmt.Sprintf("rate-limited: the canvas delivers %d messages a minute at most", routing.CanvasRatePerMinute)
	}
	return ""
}

// delivered records how a delivery went; before is the node's status
// before it, which a failed one puts back. The caller holds the lock.
func (c *Canvas) delivered(m *Message, n *Node, err error, before Status) {
	c.unqueueLocked(m)
	if c.deleted {
		return
	}
	if err != nil {
		m.State, m.Reason = MessageDropped, "delivery failed: "+err.Error()
		c.recordLocked(m)
		c.settleLocked(m.ID, "", "it was not delivered: "+err.Error())
		if st := c.states[n.ID]; st != nil && st.cause == m {
			st.cause = nil
			if st.status.State == StateBusy && st.status.Detail == "delivered" {
				c.setStatusLocked(n.ID, before.State, before.Detail)
			}
		}
		if m.Edge != "" {
			c.failures[m.Edge]++
			if c.failures[m.Edge] >= breakerFailures {
				c.failures[m.Edge] = 0
				c.breakLocked(m.Edge, err)
			}
		}
		return
	}
	t := time.Now()
	c.rates[""] = append(c.rates[""], t)
	if m.Edge != "" {
		c.rates[m.Edge] = append(c.rates[m.Edge], t)
		c.failures[m.Edge] = 0
	}
	m.State, m.DeliveredAt = MessageDelivered, now()
	c.recordLocked(m)
}

// breakLocked turns an edge off whose deliveries keep failing.
func (c *Canvas) breakLocked(id string, cause error) {
	e := c.doc.edge(id)
	if e == nil {
		return
	}
	e.Mode = ModeOff
	c.doc.Rev++
	c.doc.UpdatedAt = now()
	c.touchLocked()
	copied := *e
	c.hub.publish(map[string]any{"type": "ops", "rev": c.doc.Rev, "actor": Actor{Kind: "system"},
		"changes": []Change{{Op: "edge.update", Edge: &copied}}})
	c.noticeLocked("error", fmt.Sprintf("An edge was turned off: its last %d deliveries failed (%v).", breakerFailures, cause), "", id)
}

// approve has a message that waits for approval go, with text in place of
// its own when text is not nil.
func (c *Canvas) approve(id string, text *string) (*Message, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	m := c.pendingLocked(id)
	if m == nil {
		return nil, errNotFound("that message no longer waits")
	}
	if m.State != MessageAwaiting {
		return nil, errConflict("that message does not wait for approval")
	}
	if text != nil {
		if len(*text) > maxPaste {
			return nil, errInvalid("the text is too long")
		}
		m.Text = *text
		c.longLocked(m)
	}
	m.State, m.Reason = MessagePending, ""
	c.pendingDirty = true
	c.scheduleSaveLocked()
	c.recordLocked(m)
	c.kickDispatch()
	return m.preview(), nil
}

// drop drops a message that waits.
func (c *Canvas) drop(id string) error {
	c.mu.Lock()
	defer c.mu.Unlock()
	m := c.pendingLocked(id)
	if m == nil {
		return errNotFound("that message no longer waits")
	}
	if m.State == messageDelivering {
		return errConflict("that message is being delivered")
	}
	c.unqueueLocked(m)
	m.State, m.Reason = MessageDropped, "dropped by the user"
	c.recordLocked(m)
	c.settleLocked(m.ID, "", "the user dropped it")
	return nil
}

// message finds a message, whole.
func (c *Canvas) message(id string) (*Message, error) {
	c.mu.Lock()
	defer c.mu.Unlock()
	for i := len(c.messages) - 1; i >= 0; i-- {
		if c.messages[i].ID == id {
			copied := *c.messages[i]
			return &copied, nil
		}
	}
	return nil, errNotFound("no such message")
}

// maxPaste bounds what is sent to a node at once.
const maxPaste = 1 << 20

// sendRequest is text, or keys, for a node, straight from the user or an
// agent.
type sendRequest struct {
	Text string `json:"text"`
	// Submit presses Enter after the text, in a terminal: the default.
	Submit *bool `json:"submit"`
	// Keys are pressed after the text, as tmux's send-keys names them.
	Keys []string `json:"keys"`
	// Deliver is queue (when the node is idle) or now; When is its name
	// in the agents' tools.
	Deliver string `json:"deliver"`
	When    string `json:"when"`
	// Force hands the text to an agent at work, as deliver now does.
	Force bool `json:"force"`
	// Wait, of a node's send, waits for the target's answer to the text,
	// TimeoutS seconds at most (600 by default): the send answers it.
	Wait     bool    `json:"wait"`
	TimeoutS float64 `json:"timeout_s"`
}

// send has text, or keys, go to a node; actor is user or the node that
// sends it.
func (c *Canvas) send(actor, to string, req sendRequest) (*Message, error) {
	m, _, err := c.sendWaiting(actor, to, req)
	return m, err
}

// sendWaiting is send; for a node's send that waits (req.Wait), it gives
// what waits for the target's answer too.
func (c *Canvas) sendWaiting(actor, to string, req sendRequest) (*Message, *replyWait, error) {
	deliver := req.Deliver
	if deliver == "" {
		deliver = req.When
	}
	if req.Force {
		deliver = DeliverNow
	}
	switch deliver {
	case "", "idle", DeliverQueue:
		deliver = DeliverQueue
	case DeliverNow:
	default:
		return nil, nil, errInvalid("deliver must be queue or now")
	}
	if len(req.Text) > maxPaste {
		return nil, nil, errInvalid("the text is too long")
	}
	c.mu.Lock()
	n, st := c.doc.node(to), c.states[to]
	if n == nil || st == nil {
		c.mu.Unlock()
		return nil, nil, errNotFound("no such node")
	}
	if n.Kind != KindTerminal && n.Kind != KindAgent {
		c.mu.Unlock()
		return nil, nil, errInvalid("«" + n.Title + "» takes no input")
	}
	if len(req.Keys) != 0 {
		if n.Kind != KindTerminal {
			c.mu.Unlock()
			return nil, nil, errInvalid("keys go to terminals only")
		}
		ti, _ := st.inst.(*terminalInstance)
		c.mu.Unlock()
		if req.Text != "" {
			if _, err := c.send(actor, to, sendRequest{Text: req.Text, Submit: req.Submit, Deliver: deliver}); err != nil {
				return nil, nil, err
			}
		}
		if ti == nil {
			return nil, nil, errConflict("«" + n.Title + "» has no shell running")
		}
		if err := ti.keys(req.Keys); err != nil {
			return nil, nil, errInvalid(err.Error())
		}
		return nil, nil, nil
	}
	if req.Text == "" {
		c.mu.Unlock()
		return nil, nil, errInvalid("text is required")
	}
	if actor == to {
		c.mu.Unlock()
		return nil, nil, errInvalid("that is your own node: answer instead")
	}
	if req.Wait && actor != "user" && c.waitsOnLocked(to, actor) {
		c.mu.Unlock()
		return nil, nil, errConflict("«" + n.Title + "» waits for your answer to what it sent you: answer it first, or send without wait")
	}
	chain, hops := newID("c_", 10), 0
	from := Port{Node: actor, Port: "send"}
	text := req.Text
	if actor != "user" {
		if sender := c.states[actor]; sender != nil && sender.cause != nil {
			chain, hops = sender.cause.Chain, sender.cause.Hops+1
		}
		if limit := c.doc.Settings.Routing.MaxHops; hops > limit {
			c.mu.Unlock()
			return nil, nil, errConflict(fmt.Sprintf("the chain of messages went over %d hops: a loop?", limit))
		}
		// An agent is told which node writes to it, as along an edge.
		if sender := c.doc.node(actor); sender != nil && c.answersLocked(n) {
			text = renderMessage("", messageVars{text: text, from: sender}, true)
		}
	}
	m := &Message{
		ID: newID("m_", 10), Chain: chain, Hops: hops, From: from, To: Port{to, "in"}, At: now(),
		Text: text, Deliver: deliver, Actor: actor, NoSubmit: req.Submit != nil && !*req.Submit, State: MessagePending,
	}
	var w *replyWait
	if req.Wait && actor != "user" {
		w = &replyWait{from: actor, to: to, done: make(chan struct{})}
		c.waits[m.ID] = w
	}
	c.longLocked(m)
	c.enqueueLocked(m)
	// The dispatcher may be delivering it already: the preview is taken
	// before the lock is let go.
	preview := m.preview()
	c.mu.Unlock()
	return preview, w, nil
}

// opError is an error with the HTTP status that says it.
type opError struct {
	status int
	text   string
	rev    int64
}

func (e *opError) Error() string { return e.text }

func errInvalid(text string) error   { return &opError{status: 400, text: text} }
func errForbidden(text string) error { return &opError{status: 403, text: text} }
func errNotFound(text string) error  { return &opError{status: 404, text: text} }
func errConflict(text string) error  { return &opError{status: 409, text: text} }
func errLimit(text string) error     { return &opError{status: 429, text: text} }

// statusOf is the HTTP status of an error.
func statusOf(err error) int {
	var op *opError
	if errors.As(err, &op) {
		return op.status
	}
	return 500
}
