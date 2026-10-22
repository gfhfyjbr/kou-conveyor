package canvas

import (
	"net/http"
	"strings"
	"testing"
	"time"
)

// A node's agent is told of the canvas with the first prompt of its
// session, whoever writes it: the canvas, or the user in the node's chat.
func TestTheFirstPromptOfANodesSessionHasItsBrief(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Briefs")
	node := te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"Helper"}}]`).IDs["0"]
	session := sessionOf(t, c, node)

	// Written in its chat before its first run: the brief comes first, and
	// the node is told so once the run starts.
	prompt, told := te.Brief("w1", session, "hi", true)
	if !told || !strings.HasPrefix(prompt, briefStart+"Helper»") || withoutBrief(prompt) != "hi" {
		t.Fatalf("the first prompt: %q, %v", prompt, told)
	}
	if c.node(node).Runtime.Briefed {
		t.Fatal("briefed before its run started")
	}
	te.Briefed("w1", session)
	if !c.node(node).Runtime.Briefed {
		t.Fatal("not briefed once its run started")
	}

	// Told, its prompts are as written — but a first one again, as when the
	// first is edited.
	if prompt, told := te.Brief("w1", session, "again", false); told || prompt != "again" {
		t.Fatalf("a later prompt: %q, %v", prompt, told)
	}
	if prompt, told := te.Brief("w1", session, "edited", true); !told || !strings.HasPrefix(prompt, briefStart) || withoutBrief(prompt) != "edited" {
		t.Fatalf("the first prompt edited: %q, %v", prompt, told)
	}
	// One from the canvas has its brief already.
	sent := c.brief(node) + "\n\nfrom the canvas"
	if prompt, told := te.Brief("w1", session, sent, true); !told || prompt != sent {
		t.Fatalf("a prompt with its brief: %q, %v", prompt, told)
	}
	// The canvas does not brief it again.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+node+"/send", `{"text":"look after the children"}`, "", nil)
	eventually(t, 5*time.Second, "the message", func() bool { return len(te.host.promptsOf(session)) == 1 })
	if text := te.host.promptsOf(session)[0].text; strings.Contains(text, briefStart) || !strings.HasSuffix(text, "look after the children") {
		t.Fatalf("the canvas's message: %q", text)
	}

	// The sessions of no canvas, and an engine of none, are left as they are.
	if prompt, told := te.Brief("w1", "0a1b2c3d-0000-4000-8000-00000000ffff", "hi", true); told || prompt != "hi" {
		t.Fatalf("a session of no canvas: %q, %v", prompt, told)
	}
	var none *Engine
	if prompt, told := none.Brief("w1", session, "hi", true); told || prompt != "hi" {
		t.Fatalf("no engine: %q, %v", prompt, told)
	}
	none.Briefed("w1", session)
}
