package canvas

import (
	"encoding/json/v2"
	"net/http"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
)

// history writes a kou agent's session as its runner does: each prompt it
// ran under the prompt's ID, and the turn that answered it.
type history struct {
	t       *testing.T
	store   *localfile.Store
	session session.ID
	last    session.TurnID
}

func newHistory(t *testing.T, dir, id string) *history {
	t.Helper()
	store, err := localfile.New(dir)
	if err != nil {
		t.Fatal(err)
	}
	if _, err := store.Create(t.Context(), session.ID(id)); err != nil {
		t.Fatal(err)
	}
	return &history{t: t, store: store, session: session.ID(id)}
}

// ran records a run of the prompt with the given ID.
func (h *history) ran(id, text string) {
	h.t.Helper()
	payload, err := json.Marshal(text)
	if err != nil {
		h.t.Fatal(err)
	}
	if err := h.store.AppendInput(h.t.Context(), h.session, inbox.Input{ID: inbox.ID(id), Kind: inbox.InputExternal, Payload: payload}); err != nil {
		h.t.Fatal(err)
	}
	turn := session.TurnID("turn-" + id)
	if err := h.store.AppendTurn(h.t.Context(), h.session, session.Turn{ID: turn, PreviousTurnID: h.last, Type: session.TurnRegular}); err != nil {
		h.t.Fatal(err)
	}
	h.last = turn
}

// A node puts the prompt it gave a kou agent otherwise — the agent's model
// refused it —, without starting over: its text takes the prompt's place,
// in the session as it was before the prompt, and its answer goes where
// the first one's would have.
func TestANodeReplacesThePromptItGaveAnAgent(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Replace")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Lead"}},
		{"op":"node.add","node":{"kind":"agent","title":"Helper"}},
		{"op":"node.add","node":{"kind":"agent","title":"Other"}}
	]`)
	lead, helper, other := res.IDs["0"], res.IDs["1"], res.IDs["2"]
	sh := sessionOf(t, c, helper)
	talk, otherTalk := te.token(t, c, lead, ScopeTalk), te.token(t, c, other, ScopeTalk)
	h := newHistory(t, te.host.ws.SessionDir, sh)
	replace := func(text, token string, want int, says string) {
		t.Helper()
		status, body := te.call(t, "POST", "/api/canvas/nodes/"+helper+"/send", `{"text":"`+text+`","replace":true}`, token)
		if status != want || !strings.Contains(string(body), says) {
			t.Fatalf("a replace with %q: %d %s", text, status, body)
		}
	}

	// Nothing given it yet, there is nothing to replace.
	replace("try this", talk, http.StatusConflict, "no prompt of yours to replace")

	// The lead asks, and the helper's model refuses.
	asked := te.sendAsync(helper, `{"text":"do the risky thing","wait":true}`, talk)
	eventually(t, 5*time.Second, "the helper asked", func() bool { return len(te.host.promptsOf(sh)) == 1 })
	first := te.host.promptsOf(sh)[0]
	h.ran(first.id, first.text)
	te.RunStarted("w1", sh)
	te.RunFinished("w1", sh, "", "refused", []string{first.id})
	if got := receive(t, asked); got.Answered || !strings.Contains(got.Reason, "refused") {
		t.Fatalf("the refused send answered %+v", got)
	}

	// Keys replace nothing; an agent at work keeps its prompt; another node
	// has no prompt of its own to replace.
	if status, body := te.call(t, "POST", "/api/canvas/nodes/"+helper+"/send", `{"text":"x","keys":["Enter"],"replace":true}`, talk); status != http.StatusBadRequest {
		t.Fatalf("a replace with keys: %d %s", status, body)
	}
	te.host.setRunning(sh, true)
	replace("do the careful thing", talk, http.StatusConflict, "is at work")
	te.host.setRunning(sh, false)
	replace("mine", otherTalk, http.StatusConflict, "no prompt of yours to replace")

	// The lead puts it otherwise: the helper runs the new text in place of
	// the refused prompt, from the session as it was before it — the brief
	// is the server's to give again, should the session be left without a
	// prompt —, and the answer is the lead's.
	asked = te.sendAsync(helper, `{"text":"do the careful thing","replace":true,"wait":true}`, talk)
	eventually(t, 5*time.Second, "the prompt replaced", func() bool { return len(te.host.promptsOf(sh)) == 2 })
	second := te.host.promptsOf(sh)[1]
	if second.rewind != first.id || second.id == first.id || strings.HasPrefix(second.text, briefStart) ||
		second.text != "[canvas] from «Lead» ("+lead+"):\ndo the careful thing" {
		t.Fatalf("the replacing prompt: %+v", second)
	}
	h.ran(second.id, second.text)
	te.answer(t, sh, "done carefully")
	if got := receive(t, asked); !got.Answered || got.Answer != "done carefully" {
		t.Fatalf("the replacing send answered %+v", got)
	}

	// Once another prompt came after the lead's, the lead's is not the one
	// to take back; the other node's is.
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+helper+"/send", `{"text":"and mine"}`, otherTalk, nil)
	eventually(t, 5*time.Second, "the other's prompt", func() bool { return len(te.host.promptsOf(sh)) == 3 })
	third := te.host.promptsOf(sh)[2]
	h.ran(third.id, third.text)
	te.answer(t, sh, "yours")
	replace("again", talk, http.StatusConflict, "another prompt after yours")
	replace("mine, otherwise", otherTalk, http.StatusOK, "")
	eventually(t, 5*time.Second, "the other's prompt replaced", func() bool { return len(te.host.promptsOf(sh)) == 4 })
	if fourth := te.host.promptsOf(sh)[3]; fourth.rewind != third.id {
		t.Fatalf("the other's replacing prompt: %+v", fourth)
	}
	// Plain sends stay plain.
	for _, p := range te.host.promptsOf(sh)[:3] {
		if p.rewind != "" && p.id != second.id {
			t.Fatalf("a prompt that replaced nothing: %+v", p)
		}
	}
}
