package canvas

import (
	"encoding/json/v2"
	"fmt"
	"io"
	"net/http"
	"strings"
	"testing"
	"time"
)

// sent is what a node's send answers.
type sent struct {
	Message  *Message `json:"message"`
	Answered bool     `json:"answered"`
	Answer   string   `json:"answer"`
	Reason   string   `json:"reason"`
	TimedOut bool     `json:"timed_out"`
	Status   Status   `json:"status"`
	// code is the HTTP status; err, what failed to make the request.
	code int
	err  error
}

// sendAsync has a node send text to another, in the background: what the
// send answers comes on the channel.
func (te *testEngine) sendAsync(to, body, token string) <-chan sent {
	out := make(chan sent, 1)
	go func() {
		var got sent
		req, err := http.NewRequest("POST", te.srv.URL+"/api/canvas/nodes/"+to+"/send", strings.NewReader(body))
		if err != nil {
			got.err = err
			out <- got
			return
		}
		req.Header.Set("Content-Type", "application/json")
		req.Header.Set("Authorization", "Bearer "+token)
		res, err := te.srv.Client().Do(req)
		if err != nil {
			got.err = err
			out <- got
			return
		}
		defer res.Body.Close()
		data, _ := io.ReadAll(res.Body)
		got.code = res.StatusCode
		if err := json.Unmarshal(data, &got); err != nil && res.StatusCode == http.StatusOK {
			got.err = err
		}
		out <- got
	}()
	return out
}

// receive waits for what a send answers.
func receive(t *testing.T, ch <-chan sent) sent {
	t.Helper()
	select {
	case got := <-ch:
		if got.err != nil {
			t.Fatal(got.err)
		}
		return got
	case <-time.After(30 * time.Second):
		t.Fatal("the send did not answer")
	}
	return sent{}
}

func TestTheBriefSaysWhoToMessageAndHow(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Brief")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Lead"}},
		{"op":"node.add","node":{"kind":"agent","title":"Helper"}},
		{"op":"node.add","node":{"preset":"manual","title":"Issues"}},
		{"op":"node.add","node":{"preset":"note","title":"Notes"}},
		{"op":"node.add","node":{"kind":"agent","title":"Watcher","access":"observe","config":{"output":"all"}}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}}
	]`)
	lead, helper, watcher := res.IDs["0"], res.IDs["1"], res.IDs["4"]

	// The nodes it can message are named with their IDs — not the source,
	// nor the note —, where its answers go, and the one call that sends.
	brief := c.brief(lead)
	t.Log(brief)
	for _, want := range []string{
		"\nNodes you can message: «Helper» (" + helper + ", kou agent), «Watcher» (" + watcher + ", kou agent).\n",
		"a node's message is answered back to that node, and on along your edges (your edges take it to «Helper» (" + helper + "))",
		"the user's prompts (those without a [canvas] line) and replies are answered to the user alone — that answer goes to no node.\n",
		`call the kou-canvas tool CanvasSend once — {"node":"<id or title>","text":"…","wait":true}`,
		`kou-canvas send <node> "text" --wait`,
	} {
		if !strings.Contains(brief, want) {
			t.Errorf("the brief has no %q", want)
		}
	}
	if strings.Contains(brief, "\n\n") || len(brief) > maxBrief {
		t.Fatalf("the brief is %d bytes, or has a blank line", len(brief))
	}

	// One that cannot send is not told how to; every answer of one whose
	// output is all goes out.
	brief = c.brief(watcher)
	if strings.Contains(brief, "CanvasSend") || !strings.Contains(brief, "\nThe other nodes: «Lead»") ||
		!strings.Contains(brief, "Every answer you end a turn with goes out — nothing is wired to your output yet —, the user's too") {
		t.Fatalf("the watcher's brief:\n%s", brief)
	}

	// On a crowded canvas, the nodes past what the brief has room for are
	// counted.
	var ops []string
	for i := range 40 {
		ops = append(ops, fmt.Sprintf(`{"op":"node.add","node":{"kind":"agent","title":"Worker number %d with a long title"}}`, i))
	}
	te.ops(t, id, "["+strings.Join(ops, ",")+"]")
	brief = c.brief(lead)
	if len(brief) > maxBrief || !strings.Contains(brief, " more, which CanvasView lists.\n") || !strings.Contains(brief, "CanvasSend once") {
		t.Fatalf("the crowded brief, %d bytes:\n%s", len(brief), brief)
	}
}

func TestAnswersToTheUserStayWithTheNode(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Answers")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"manual","title":"Issues"}},
		{"op":"node.add","node":{"kind":"agent","title":"Coder"}},
		{"op":"node.add","node":{"kind":"agent","title":"Reviewer"}},
		{"op":"edge.add","edge":{"from":{"node":"$0"},"to":{"node":"$1"}}},
		{"op":"edge.add","edge":{"from":{"node":"$1"},"to":{"node":"$2"}}}
	]`)
	source, coder := res.IDs["0"], res.IDs["1"]
	sc, sr := sessionOf(t, c, coder), sessionOf(t, c, res.IDs["2"])

	// What the user asks the coder, the coder answers to the user alone.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+coder+"/send", `{"text":"what is 2+2?"}`, "", nil)
	eventually(t, 5*time.Second, "the coder asked", func() bool { return len(te.host.promptsOf(sc)) == 1 })
	te.answer(t, sc, "4")
	// So does a run of the user's that the canvas did not start.
	te.RunFinished("w1", sc, "typed by hand", "done", []string{"a-prompt-of-the-users"})
	time.Sleep(200 * time.Millisecond)
	if prompts := te.host.promptsOf(sr); len(prompts) != 0 {
		t.Fatalf("answers to the user went along the edge: %+v", prompts)
	}

	// What comes along an edge, it answers along its edges.
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+source+"/fire", `{"text":"issue 7"}`, "", nil)
	eventually(t, 5*time.Second, "the coder given the issue", func() bool { return len(te.host.promptsOf(sc)) == 2 })
	te.answer(t, sc, "fixed 7")
	eventually(t, 5*time.Second, "the reviewer given the fix", func() bool { return len(te.host.promptsOf(sr)) == 1 })
	if text := te.host.promptsOf(sr)[0].text; !strings.HasSuffix(text, "\n\n[canvas] from «Coder» ("+coder+"):\nfixed 7") {
		t.Fatalf("the reviewer was given %q", text)
	}

	// Its output all, the answers to the user go out too.
	te.ops(t, id, `[{"op":"node.update","id":"`+coder+`","set":{"config":{"output":"all"}}}]`)
	te.RunFinished("w1", sc, "for everyone", "done", []string{"another-of-the-users"})
	eventually(t, 5*time.Second, "the answer to the user put out", func() bool { return len(te.host.promptsOf(sr)) == 2 })

	// Explicit, none does: only what it emits itself.
	te.ops(t, id, `[{"op":"node.update","id":"`+coder+`","set":{"config":{"output":"explicit"}}}]`)
	te.must(t, http.StatusOK, "POST", "/api/w/w1/canvases/"+id+"/nodes/"+source+"/fire", `{"text":"issue 8"}`, "", nil)
	eventually(t, 5*time.Second, "the coder given the next issue", func() bool { return len(te.host.promptsOf(sc)) == 3 })
	te.answer(t, sc, "fixed 8")
	time.Sleep(200 * time.Millisecond)
	if prompts := te.host.promptsOf(sr); len(prompts) != 2 {
		t.Fatalf("an explicit node's answer went out: %+v", prompts)
	}
}

func TestANodeAsksAnotherAndHasItsAnswer(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Ask")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Lead"}},
		{"op":"node.add","node":{"kind":"agent","title":"Helper"}},
		{"op":"node.add","node":{"kind":"agent","title":"Bystander"}},
		{"op":"edge.add","edge":{"from":{"node":"$1"},"to":{"node":"$2"}}}
	]`)
	lead, helper := res.IDs["0"], res.IDs["1"]
	sl, sh, sb := sessionOf(t, c, lead), sessionOf(t, c, helper), sessionOf(t, c, res.IDs["2"])
	talk := te.token(t, c, lead, ScopeTalk)

	// A send that waits — the node named by its title — has the answer.
	asked := te.sendAsync("helper", `{"text":"what time is it?","wait":true}`, talk)
	eventually(t, 5*time.Second, "the helper asked", func() bool { return len(te.host.promptsOf(sh)) == 1 })
	if text := te.host.promptsOf(sh)[0].text; !strings.HasSuffix(text, "\n\n[canvas] from «Lead» ("+lead+"):\nwhat time is it?") {
		t.Fatalf("the helper was told %q", text)
	}
	te.answer(t, sh, "noon")
	if got := receive(t, asked); got.code != http.StatusOK || !got.Answered || got.Answer != "noon" || got.TimedOut || got.Status.State != StateIdle {
		t.Fatalf("the send answered %+v", got)
	}
	// The answer to a node goes along the helper's edges too, and comes
	// back as no reply: the send had it.
	eventually(t, 5*time.Second, "the bystander given the answer", func() bool { return len(te.host.promptsOf(sb)) == 1 })
	time.Sleep(200 * time.Millisecond)
	if prompts := te.host.promptsOf(sl); len(prompts) != 0 {
		t.Fatalf("the lead was given %+v", prompts)
	}

	// A send that does not wait has the answer come as a reply, which the
	// lead answers to no one.
	te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+helper+"/send", `{"text":"and the date?"}`, talk, nil)
	eventually(t, 5*time.Second, "the helper asked again", func() bool { return len(te.host.promptsOf(sh)) == 2 })
	te.answer(t, sh, "October 3")
	eventually(t, 5*time.Second, "the lead given the reply", func() bool { return len(te.host.promptsOf(sl)) == 1 })
	if text := te.host.promptsOf(sl)[0].text; !strings.HasSuffix(text, "\n\n[canvas] reply from «Helper» ("+helper+"):\nOctober 3") {
		t.Fatalf("the reply %q", text)
	}
	te.answer(t, sl, "thanks")
	time.Sleep(200 * time.Millisecond)
	if prompts := te.host.promptsOf(sh); len(prompts) != 2 {
		t.Fatalf("the answer to a reply went on: %+v", prompts)
	}

	// A wait that times out leaves the answer to come as a reply.
	asked = te.sendAsync(helper, `{"text":"slowly","wait":true,"timeout_s":1}`, talk)
	if got := receive(t, asked); got.code != http.StatusOK || !got.TimedOut || got.Answered {
		t.Fatalf("the send answered %+v", got)
	}
	te.answer(t, sh, "at last")
	eventually(t, 5*time.Second, "the late answer as a reply", func() bool { return len(te.host.promptsOf(sl)) == 2 })
	if text := te.host.promptsOf(sl)[1].text; text != "[canvas] reply from «Helper» ("+helper+"):\nat last" {
		t.Fatalf("the late reply %q", text)
	}

	// A run that fails answers the send that waits with why.
	asked = te.sendAsync(helper, `{"text":"break","wait":true}`, talk)
	eventually(t, 5*time.Second, "the helper asked to break", func() bool { return len(te.host.promptsOf(sh)) == 4 })
	prompts := te.host.promptsOf(sh)
	te.RunFinished("w1", sh, "", "failed", []string{prompts[len(prompts)-1].id})
	if got := receive(t, asked); got.Answered || !strings.Contains(got.Reason, "failed") {
		t.Fatalf("the send answered %+v", got)
	}

	// Two nodes that would wait for each other are told so.
	helperTalk := te.token(t, c, helper, ScopeTalk)
	asked = te.sendAsync(lead, `{"text":"question","wait":true}`, helperTalk)
	eventually(t, 5*time.Second, "the lead asked", func() bool { return len(te.host.promptsOf(sl)) == 3 })
	if status, body := te.call(t, "POST", "/api/canvas/nodes/"+helper+"/send", `{"text":"counter","wait":true}`, talk); status != http.StatusConflict {
		t.Fatalf("a wait for a node that waits: %d %s", status, body)
	}
	te.answer(t, sl, "answered")
	if got := receive(t, asked); got.Answer != "answered" {
		t.Fatalf("the helper's send answered %+v", got)
	}
}
