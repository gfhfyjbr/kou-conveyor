package cockpit

import (
	"encoding/json/v2"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
)

func queueTexts(q *Queue) string {
	var parts []string
	for _, item := range q.Items {
		text := item.Text
		if item.Forced {
			text = "!" + text
		}
		parts = append(parts, text)
	}
	if q.Paused {
		parts = append(parts, "(paused)")
	}
	return strings.Join(parts, " ")
}

func TestQueueKeepsForcedPromptsFirst(t *testing.T) {
	var q Queue
	a, b, c := q.Add("a", ""), q.Add("b", ""), q.Add("c", "")
	if _, ok := q.Force(c.ID); !ok {
		t.Fatal("c was not forced")
	}
	q.Force(a.ID)
	if got := queueTexts(&q); got != "!c !a b" {
		t.Fatalf("queue = %s", got)
	}
	if q.Forced() != 2 || q.Queued() != 1 {
		t.Fatalf("forced %d, queued %d", q.Forced(), q.Queued())
	}
	if q.Edit(c.ID, "changed") {
		t.Fatal("a forced prompt was edited")
	}
	d := q.Add("d", "")
	q.Move(d.ID, 0)
	if got := queueTexts(&q); got != "!c !a d b" {
		t.Fatalf("after moving d first: %s", got)
	}
	q.Move(d.ID, 9)
	q.Edit(b.ID, "B")
	if got := queueTexts(&q); got != "!c !a B d" {
		t.Fatalf("after moving d last: %s", got)
	}
	// A runner that cannot take a prompt leaves it first in line.
	q.Unforce(a.ID)
	if got := queueTexts(&q); got != "!c a B d" {
		t.Fatalf("after unforcing a: %s", got)
	}
	if _, index, _ := q.Get(d.ID); index != 2 {
		t.Fatalf("d is at %d", index)
	}
	item, _ := q.Remove(b.ID)
	q.Insert(1, item)
	if got := queueTexts(&q); got != "!c a B d" {
		t.Fatalf("after taking B out and back: %s", got)
	}
	// The forced prompt belongs to the run that goes on.
	if _, ok := q.Next(); ok {
		t.Fatal("the next prompt ran while one was forced")
	}
}

func TestQueueDeliversForcedPromptsTheRunnerRecorded(t *testing.T) {
	var q Queue
	forced, _ := q.Force(q.Add("use pnpm", "").ID)
	q.Add("then commit", "")
	tr := NewTranscript()
	if got := q.Delivered(tr); got != nil {
		t.Fatalf("delivered %v before the runner recorded it", got)
	}
	payload, _ := json.Marshal("use pnpm")
	line, _ := json.Marshal(sessionstore.Item{Sequence: 1, RecordedAt: time.Now(), Kind: sessionstore.ItemInput, Data: inbox.Input{
		ID: inbox.ID(forced.ID), Kind: inbox.InputExternal, Payload: payload, Delivery: inbox.DeliverAfterTools,
	}})
	entries, err := tr.Apply(line)
	if err != nil || len(entries) != 1 || !entries[0].Forced {
		t.Fatalf("entries = %v, %v", entries, err)
	}
	delivered := q.Delivered(tr)
	if len(delivered) != 1 || delivered[0].ID != forced.ID || queueTexts(&q) != "then commit" {
		t.Fatalf("delivered %v, left %s", delivered, queueTexts(&q))
	}
}

func TestQueueSettlesWhenTheRunEnds(t *testing.T) {
	for _, outcome := range []string{"done", "stopped", "failed"} {
		t.Run(outcome, func(t *testing.T) {
			var q Queue
			q.Add("next", "")
			q.Force(q.Add("unread", "").ID)
			q.Settle(outcome)
			want := "unread next"
			if outcome != "done" {
				want += " (paused)"
			}
			if got := queueTexts(&q); got != want {
				t.Fatalf("queue = %s, want %s", got, want)
			}
			item, ok := q.Next()
			if ok != (outcome == "done") || ok && item.Text != "unread" {
				t.Fatalf("next = %v, %t", item, ok)
			}
		})
	}
}

func TestQueueEmptiedIsNoLongerPaused(t *testing.T) {
	var q Queue
	item := q.Add("a", "")
	q.Settle("stopped")
	if !q.Paused {
		t.Fatal("a stopped run did not pause the queue")
	}
	q.Remove(item.ID)
	if q.Paused {
		t.Fatal("an empty queue stayed paused")
	}
	// An empty queue a run ends with has nothing to pause.
	q.Settle("failed")
	if q.Paused {
		t.Fatal("an empty queue was paused")
	}
	q.Add("b", "")
	q.Add("c", "")
	q.Paused = true
	clone := q.Clone()
	q.Clear()
	if q.Paused || len(q.Items) != 0 || len(clone.Items) != 2 || !clone.Paused {
		t.Fatalf("queue %s, clone %s", queueTexts(&q), queueTexts(&clone))
	}
}
