package canvas

import (
	"encoding/json/v2"
	"io"
	"net/http"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// A run that ends without an answer — its model refused, it was stopped or
// failed — ends a wait for the answer: none is to come of it.
func TestAWaitForAnAnswerEndsWithARunThatGaveNone(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Refusals")
	res := te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"Lead"}}]`)
	build := te.token(t, c, res.IDs["0"], ScopeBuild)
	var made Result
	te.must(t, http.StatusOK, "POST", "/api/canvas/ops", `{"ops":[{"op":"node.add","node":{"preset":"agent","title":"A"}}]}`, build, &made)
	a := made.IDs["0"]
	sa := sessionOf(t, c, a)
	send := func(text string, prompts int) string {
		t.Helper()
		te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+a+"/send", `{"text":"`+text+`"}`, build, nil)
		eventually(t, 5*time.Second, "the prompt "+text, func() bool { return len(te.host.promptsOf(sa)) == prompts })
		te.RunStarted("w1", sa)
		return te.host.promptsOf(sa)[prompts-1].id
	}

	get := func(path string, v any) <-chan struct{} {
		done := make(chan struct{})
		go func() {
			defer close(done)
			req, _ := http.NewRequest("GET", te.srv.URL+path, nil)
			req.Header.Set("Authorization", "Bearer "+build)
			res, err := te.srv.Client().Do(req)
			if err != nil {
				t.Errorf("%s: %v", path, err)
				return
			}
			defer res.Body.Close()
			body, _ := io.ReadAll(res.Body)
			if res.StatusCode != http.StatusOK {
				t.Errorf("%s: %d %s", path, res.StatusCode, body)
			} else if err := json.Unmarshal(body, v); err != nil {
				t.Errorf("%s: %v", path, err)
			}
		}()
		return done
	}
	ended := func(done <-chan struct{}) {
		t.Helper()
		select {
		case <-done:
		case <-time.After(5 * time.Second):
			t.Fatal("the wait did not end")
		}
	}
	waitAny := func(query string) WaitAnyResult {
		t.Helper()
		var got WaitAnyResult
		ended(get("/api/canvas/wait?nodes="+a+"&"+query, &got))
		return got
	}

	// Its model refuses to the end: the wait for its answer ends, and says
	// why no answer came.
	prompt := send("your part", 1)
	var got WaitAnyResult
	pending := get("/api/canvas/wait?until=answer&timeout=5&nodes="+a, &got)
	time.Sleep(50 * time.Millisecond)
	te.RunFinished("w1", sa, "", "refused", []string{prompt})
	ended(pending)
	if got.Fired != a || got.TimedOut || len(got.Nodes) != 1 || got.Next != a+":0/1" ||
		got.Nodes[0].NoAnswer != "«A»'s model refused to answer" || got.Nodes[0].Status.Detail != "refused" {
		t.Fatalf("a refusal: %+v", got)
	}
	// The cursor has it: nothing happened since.
	if got = waitAny("until=answer&timeout=1&after=" + got.Next); !got.TimedOut || got.Fired != "" {
		t.Fatalf("after the refusal: %+v", got)
	}
	// A run of the user's that answers nothing is not the canvas's to wait
	// for, as its answer would not be.
	te.RunStarted("w1", sa)
	te.RunFinished("w1", sa, "", "done", nil)
	if got = waitAny("until=answer&timeout=1&after=" + a + ":0/1"); !got.TimedOut || got.Next != a+":0/1" {
		t.Fatalf("the user's run: %+v", got)
	}
	// Asked again, it answers: the answer ends the wait, and the refusal is
	// behind it.
	prompt = send("your part, again", 2)
	te.RunFinished("w1", sa, "done now", "done", []string{prompt})
	if got = waitAny("until=answer&timeout=5&after=" + a + ":0/1"); got.Fired != a || got.Next != a+":1/1" || got.Nodes[0].NoAnswer != "" {
		t.Fatalf("the answer: %+v", got)
	}

	// A wait for one node's next output ends so too: its run failed.
	prompt = send("and more", 3)
	var one WaitResult
	pending = get("/api/canvas/nodes/"+a+"/wait?until=output&timeout=5", &one)
	time.Sleep(50 * time.Millisecond)
	te.RunFinished("w1", sa, "", "failed", []string{prompt})
	ended(pending)
	if one.TimedOut || one.Outputs != 1 || one.NoAnswer != "«A»'s run failed" {
		t.Fatalf("a wait for an output of a run that failed: %+v", one)
	}
	// A cursor of before the runs without an answer has them.
	if got = waitAny("until=answer&timeout=1&after=" + a + ":1"); got.Fired != a || got.Next != a+":1/2" {
		t.Fatalf("an older cursor: %+v", got)
	}
}

func TestRefusedIsTheModelsRefusalOfTheLastPrompt(t *testing.T) {
	user := &cockpit.Entry{Kind: cockpit.KindUser, Text: "do it"}
	refused := &cockpit.Entry{Kind: cockpit.KindNotice, Text: cockpit.RefusedNotice}
	cut := &cockpit.Entry{Kind: cockpit.KindNotice, Text: "Response cut off at the output token limit"}
	thought := &cockpit.Entry{Kind: cockpit.KindReasoning, Text: "hm"}
	for _, tc := range []struct {
		entries []*cockpit.Entry
		want    bool
	}{
		{[]*cockpit.Entry{user, thought, refused}, true},
		{[]*cockpit.Entry{user, refused, thought, refused}, true},
		{[]*cockpit.Entry{refused, user}, false},
		{[]*cockpit.Entry{user, cut}, false},
		{nil, false},
	} {
		if got := Refused(tc.entries); got != tc.want {
			t.Fatalf("Refused(%d entries) = %v, want %v", len(tc.entries), got, tc.want)
		}
	}
}
