package canvas

import (
	"net/http"
	"testing"
	"time"
)

// Editing a prompt of a node's agent rewinds its session: the answer of a
// run the rewind removed is no longer the node's to give.
func TestARewoundAgentForgetsItsDiscardedAnswer(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Rewound")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"manual","title":"Issues"}},
		{"op":"node.add","node":{"kind":"agent","title":"Alpha"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}}
	]`)
	source, alpha := res.IDs["0"], res.IDs["1"]
	sa := sessionOf(t, c, alpha)
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+source+"/fire", `{"text":"issue 42"}`, "", nil)
	eventually(t, 5*time.Second, "Alpha prompted", func() bool { return len(te.host.promptsOf(sa)) == 1 })
	te.RunStarted("w1", sa)
	te.answer(t, sa, "the answer of a discarded run")

	read := func() string {
		t.Helper()
		var read struct {
			Text string `json:"text"`
		}
		te.must(t, http.StatusOK, "GET", "/api/w/w1/canvases/"+id+"/nodes/"+alpha+"/read?what=answer", "", "", &read)
		return read.Text
	}
	if got := read(); got != "the answer of a discarded run" {
		t.Fatalf("Alpha's answer reads %q", got)
	}
	te.Rewound("w1", sa)
	te.RunStarted("w1", sa)
	// Its answer is what remains of its session, which holds none here.
	if got := read(); got != "" {
		t.Fatalf("after the rewind, Alpha's answer reads %q", got)
	}
	// The edited prompt is the user's, whose answer stays with the user.
	// What the canvas asks next goes along the edges again.
	te.RunFinished("w1", sa, "the answer of the edited prompt", "done", nil)
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+source+"/fire", `{"text":"issue 43"}`, "", nil)
	eventually(t, 5*time.Second, "Alpha prompted again", func() bool { return len(te.host.promptsOf(sa)) == 2 })
	te.RunStarted("w1", sa)
	te.answer(t, sa, "the answer of the rewound session")
	if got := read(); got != "the answer of the rewound session" {
		t.Fatalf("after the next message, Alpha's answer reads %q", got)
	}
	// A session of no canvas's is none of its business.
	te.Rewound("w1", "elsewhere")
}
