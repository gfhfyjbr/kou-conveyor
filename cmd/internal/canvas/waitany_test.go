package canvas

import (
	"encoding/json/v2"
	"io"
	"net/http"
	"testing"
	"time"
)

func TestAnAgentWaitsForAnyOfItsChildren(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Fan-out")
	res := te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"Lead"}},{"op":"node.add","node":{"kind":"agent","title":"Loner"}}]`)
	lead, loner := res.IDs["0"], res.IDs["1"]
	build := te.token(t, c, lead, ScopeBuild)
	var made Result
	te.must(t, http.StatusOK, "POST", "/api/canvas/ops", `{"ops":[{"op":"node.add","node":{"preset":"agent","title":"A"}},{"op":"node.add","node":{"preset":"agent","title":"B"}}]}`, build, &made)
	a, b := made.IDs["0"], made.IDs["1"]
	sa, sb := sessionOf(t, c, a), sessionOf(t, c, b)
	for _, node := range []string{a, b} {
		te.must(t, http.StatusOK, "POST", "/api/canvas/nodes/"+node+"/send", `{"text":"your part"}`, build, nil)
	}
	eventually(t, 5*time.Second, "the tasks", func() bool { return len(te.host.promptsOf(sa)) == 1 && len(te.host.promptsOf(sb)) == 1 })
	te.RunStarted("w1", sa)
	te.RunStarted("w1", sb)

	wait := func(query string) <-chan WaitAnyResult {
		done := make(chan WaitAnyResult, 1)
		go func() {
			defer close(done)
			req, _ := http.NewRequest("GET", te.srv.URL+"/api/canvas/wait?"+query, nil)
			req.Header.Set("Authorization", "Bearer "+build)
			res, err := te.srv.Client().Do(req)
			if err != nil {
				t.Errorf("wait %s: %v", query, err)
				return
			}
			defer res.Body.Close()
			body, _ := io.ReadAll(res.Body)
			var got WaitAnyResult
			if res.StatusCode != http.StatusOK {
				t.Errorf("wait %s: %d %s", query, res.StatusCode, body)
			} else if err := json.Unmarshal(body, &got); err != nil {
				t.Errorf("wait %s: %v", query, err)
			}
			done <- got
		}()
		return done
	}
	result := func(ch <-chan WaitAnyResult) WaitAnyResult {
		t.Helper()
		select {
		case got := <-ch:
			return got
		case <-time.After(5 * time.Second):
			t.Fatal("the wait did not end")
			return WaitAnyResult{}
		}
	}

	// The first answer of the children — made by the caller — ends it.
	pending := wait("until=answer&timeout=5")
	time.Sleep(50 * time.Millisecond)
	te.answer(t, sb, "B is done")
	got := result(pending)
	if got.Fired != b || got.TimedOut || len(got.Nodes) != 2 || got.Next != a+":0,"+b+":1" {
		t.Fatalf("the first answer: %+v", got)
	}
	// An answer given between two waits is not missed: the cursor has it.
	te.answer(t, sa, "A is done")
	got = result(wait("until=answer&timeout=5&after=" + got.Next))
	if got.Fired != a || got.Next != a+":1,"+b+":1" {
		t.Fatalf("the answer between two waits: %+v", got)
	}
	// All idle already: nothing to wait for.
	got = result(wait("until=idle&timeout=5&nodes=A,B"))
	if got.Fired != "" || got.TimedOut {
		t.Fatalf("idle already: %+v", got)
	}
	// The last of them to stop working ends a wait for all to be idle.
	te.RunStarted("w1", sa)
	te.RunStarted("w1", sb)
	pending = wait("until=idle&timeout=5&nodes=" + a + "," + b)
	time.Sleep(50 * time.Millisecond)
	te.RunFinished("w1", sb, "again", "done", nil)
	time.Sleep(50 * time.Millisecond)
	select {
	case got := <-pending:
		t.Fatalf("idle while A works: %+v", got)
	default:
	}
	te.RunFinished("w1", sa, "again", "done", nil)
	if got = result(pending); got.Fired != a {
		t.Fatalf("the last to be idle: %+v", got)
	}
	// A node removed ends a wait for any change.
	pending = wait("until=any&timeout=5&nodes=" + a)
	time.Sleep(50 * time.Millisecond)
	te.ops(t, id, `[{"op":"node.remove","id":"`+a+`","grace_s":0}]`)
	if got = result(pending); got.Fired != a || len(got.Nodes) != 1 || !got.Nodes[0].Gone {
		t.Fatalf("a node removed: %+v", got)
	}
	// Nothing happens — what a run answers to no node's message is no
	// output — so it times out, with the cursor to go on from.
	if got = result(wait("until=answer&timeout=1&nodes=B")); !got.TimedOut || got.Fired != "" || got.Next != b+":1" {
		t.Fatalf("timed out: %+v", got)
	}

	for _, tc := range []struct {
		query, token string
		want         int
	}{
		{"nodes=nobody", build, http.StatusNotFound},
		{"until=soon", build, http.StatusBadRequest},
		{"", te.token(t, c, loner, ScopeObserve), http.StatusBadRequest}, // no children
	} {
		if status, body := te.call(t, "GET", "/api/canvas/wait?"+tc.query+"&timeout=1", "", tc.token); status != tc.want {
			t.Fatalf("%s: %d %s, want %d", tc.query, status, body, tc.want)
		}
	}
}
