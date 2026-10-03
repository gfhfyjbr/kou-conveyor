package canvas

import (
	"context"
	"fmt"
	"slices"
	"strings"
	"time"
)

// Answers. What an agent answers at the end of a turn — a kou agent's run,
// a harness's turn — goes where the messages of the turn came from:
//
//   - what came from the canvas — along an edge, or from a node straight —
//     is answered along the node's own edges: the canvas's pipelines;
//   - a node that sent it a message straight (CanvasSend) is given the
//     answer too: its CanvasSend has it, when it waits for it, else it
//     comes to it as a message marked as a reply — unless an edge takes
//     the answer there already;
//   - what the user says to it, and the replies it is given, it answers
//     to the user alone: the answer stays in the node.
//
// The node's output setting changes what goes along the edges: all puts
// every answer out, the user's too; explicit, only what the agent emits
// itself. A shell's outputs are its commands', and always go along its
// edges; a node that waits for what a command it sent printed is given it.

// What an agent's node puts out along its edges at the end of a turn: its
// config's output.
const (
	// OutputReplies: its answers to messages from the canvas.
	OutputReplies = "replies"
	// OutputAll: every answer, the user's too.
	OutputAll = "all"
	// OutputExplicit: only what it emits itself.
	OutputExplicit = "explicit"
)

// outputOf is a node's output setting; final and both, as the settings
// were once named, are replies.
func outputOf(n *Node) string {
	switch strings.TrimSpace(n.configString("output")) {
	case OutputAll:
		return OutputAll
	case OutputExplicit:
		return OutputExplicit
	}
	return OutputReplies
}

// maxPrompts bounds the prompts of an agent's session whose messages are
// kept until they are answered: those the user drops from the queue are
// never answered.
const maxPrompts = 64

// fromCanvas reports whether a message came from the canvas — along an
// edge, or from a node straight — rather than from the user, or as a
// reply, which are answered to the user alone.
func fromCanvas(m *Message) bool { return m != nil && !m.Reply && m.Actor != "user" }

// direct reports whether a message is a node's, sent straight to its
// target: one whose answer goes back to that node.
func direct(m *Message) bool { return fromCanvas(m) && m.Edge == "" && m.Actor != "" }

// remember keeps the message an agent's prompt came from, until the run
// that answers it ends.
func (c *Canvas) remember(id, prompt string, m *Message) {
	c.mu.Lock()
	defer c.mu.Unlock()
	st := c.states[id]
	if st == nil {
		return
	}
	if st.prompts == nil {
		st.prompts = map[string]*Message{}
	}
	st.prompts[prompt] = m
	st.asked = append(st.asked, prompt)
	for len(st.asked) > maxPrompts {
		delete(st.prompts, st.asked[0])
		st.asked = st.asked[1:]
	}
}

// forget forgets a prompt that was not given after all.
func (c *Canvas) forget(id, prompt string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if st := c.states[id]; st != nil {
		st.takePrompts([]string{prompt})
	}
}

// takePrompts takes the messages the prompts of ids came from out of
// those an agent was given.
func (st *nodeState) takePrompts(ids []string) []*Message {
	var out []*Message
	for _, id := range ids {
		if m := st.prompts[id]; m != nil {
			out = append(out, m)
			delete(st.prompts, id)
		}
	}
	if len(out) > 0 {
		st.asked = slices.DeleteFunc(st.asked, func(id string) bool { return st.prompts[id] == nil })
	}
	return out
}

// replyWait is a CanvasSend that waits for the answer to its message.
type replyWait struct {
	// from sent the message to to.
	from, to string
	done     chan struct{}
	// answer is what the node answered; reason, why there is none.
	answer, reason string
}

// waitsOnLocked reports whether a node waits for the answer of another to
// what it sent it: the other waiting for its answer in turn would wait
// for good.
func (c *Canvas) waitsOnLocked(node, other string) bool {
	for _, w := range c.waits {
		if w.from == node && w.to == other {
			return true
		}
	}
	return false
}

// settleLocked gives the CanvasSend that waits for the answer to a
// message, if one does, the answer, or why there is none; it reports
// whether one waited.
func (c *Canvas) settleLocked(id, answer, reason string) bool {
	w := c.waits[id]
	if w == nil {
		return false
	}
	delete(c.waits, id)
	w.answer, w.reason = answer, reason
	close(w.done)
	return true
}

// awaitReply waits for the answer to a message a node sent. A wait that
// ends without it leaves the answer to come to the node as a reply.
func (c *Canvas) awaitReply(ctx context.Context, id string, w *replyWait, timeout time.Duration) (answer, reason string, timedOut bool, err error) {
	timer := time.NewTimer(min(max(timeout, time.Second), maxWait))
	defer timer.Stop()
	select {
	case <-w.done:
		return w.answer, w.reason, false, nil
	case <-timer.C:
		timedOut = true
	case <-ctx.Done():
		err = ctx.Err()
	}
	c.mu.Lock()
	defer c.mu.Unlock()
	select {
	case <-w.done: // it came meanwhile
		return w.answer, w.reason, false, nil
	default:
	}
	if c.waits[id] == w {
		delete(c.waits, id)
	}
	return "", "", timedOut, err
}

// answer takes in what a terminal's agent answered at the end of a turn,
// to the message delivered to it last.
func (c *Canvas) answer(id string, out Output) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if st := c.states[id]; st != nil {
		c.answerLocked(id, out, []*Message{st.cause}, "")
	}
}

// answerLocked takes in what an agent answered at the end of a turn, to
// the messages the turn answered — none or nil: the user alone. reason
// says why there is no answer, when there is none.
func (c *Canvas) answerLocked(id string, out Output, causes []*Message, reason string) {
	n, st := c.doc.node(id), c.states[id]
	if n == nil || st == nil || c.deleted {
		return
	}
	if st.cause != nil && slices.Contains(causes, st.cause) {
		st.cause = nil
	}
	var asked []*Message // the nodes' messages straight to it
	var chain *Message   // the message whose chain the answer goes on
	along := false
	for _, m := range causes {
		switch {
		case m == nil:
			continue
		case fromCanvas(m):
			chain, along = m, true
		case chain == nil || !fromCanvas(chain):
			chain = m
		}
		if direct(m) && !slices.ContainsFunc(asked, func(other *Message) bool { return other.Actor == m.Actor }) {
			asked = append(asked, m)
		}
	}
	if strings.TrimSpace(out.Text) == "" {
		if reason == "" {
			reason = "«" + n.Title + "» answered nothing"
		}
		for _, m := range asked {
			c.settleLocked(m.ID, "", reason)
		}
		return
	}
	switch outputOf(n) {
	case OutputAll:
		along = true
	case OutputExplicit:
		along = false
	}
	if out.Port == "" {
		out.Port = "out"
	}
	// A node whose CanvasSend waits has the answer from it: no edge gives
	// it the answer again.
	answered := map[string]bool{}
	for _, m := range asked {
		if c.settleLocked(m.ID, out.Text, "") {
			answered[m.Actor] = true
		}
	}
	if along {
		c.putOutLocked(id, out, chain, answered)
	}
	for _, m := range asked {
		if !answered[m.Actor] && !(along && c.edgeToLocked(id, out.Port, m.Actor)) {
			c.replyLocked(n, m, out.Text)
		}
	}
}

// edgeToLocked reports whether an edge that is not off takes what a node
// puts out on port to target.
func (c *Canvas) edgeToLocked(node, port, target string) bool {
	return slices.ContainsFunc(c.doc.Edges, func(e *Edge) bool {
		return e.From.Node == node && e.From.Port == port && e.To.Node == target && e.mode() != ModeOff
	})
}

// replyLocked gives a node that sent a message straight to another the
// answer to it, as a reply. Only agents are replied to — a shell would run
// the reply — and what they answer to a reply stays with them: the
// exchange ends there, unless they ask again.
func (c *Canvas) replyLocked(from *Node, m *Message, text string) {
	sender, st := c.doc.node(m.Actor), c.states[m.Actor]
	if sender == nil || st == nil || !c.answersLocked(sender) {
		return
	}
	hops := m.Hops + 1
	if limit := c.doc.Settings.Routing.MaxHops; hops > limit {
		c.noticeLocked("warning", fmt.Sprintf("«%s»'s answer to «%s» was dropped: its chain went over %d hops, which looks like a loop.", from.Title, sender.Title, limit), sender.ID, "")
		return
	}
	reply := &Message{
		ID: newID("m_", 10), Chain: m.Chain, Hops: hops, From: Port{from.ID, "out"}, To: Port{sender.ID, "in"}, At: now(),
		Title: "Reply", Text: headerOf("reply from", from) + text, Deliver: DeliverQueue,
		Actor: from.ID, Reply: true, InReplyTo: m.ID, State: MessagePending,
	}
	c.longLocked(reply)
	c.enqueueLocked(reply)
}

// answersLocked reports whether a node's program is an agent, which takes
// a reply as a prompt: a kou agent, a harness, an agent run in a shell.
func (c *Canvas) answersLocked(n *Node) bool {
	if n.agentish() {
		return true
	}
	st := c.states[n.ID]
	return st != nil && st.status.Agent != nil
}

// commandOutput has a shell give what its last command printed: along its
// edges, and to the node that sent the command, when it waits for it.
func (c *Canvas) commandOutput(id string, out Output) {
	c.mu.Lock()
	defer c.mu.Unlock()
	st := c.states[id]
	if st == nil {
		return
	}
	cause := st.cause
	st.cause = nil
	var answered map[string]bool
	if direct(cause) && c.settleLocked(cause.ID, out.Text, "") {
		answered = map[string]bool{cause.Actor: true}
	}
	c.putOutLocked(id, out, cause, answered)
}

// unanswered is unansweredLocked, for one that does not hold the lock.
func (c *Canvas) unanswered(id, reason string) {
	c.mu.Lock()
	defer c.mu.Unlock()
	c.unansweredLocked(id, reason)
}

// unansweredLocked is a node done with the message delivered to it last
// without an answer: what waits for one is told why.
func (c *Canvas) unansweredLocked(id, reason string) {
	st := c.states[id]
	if st == nil || st.cause == nil {
		return
	}
	cause := st.cause
	st.cause = nil
	if direct(cause) {
		c.settleLocked(cause.ID, "", reason)
	}
}
