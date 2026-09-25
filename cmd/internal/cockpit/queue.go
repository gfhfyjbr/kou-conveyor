package cockpit

import (
	"slices"
	"time"
	"uuid"
)

// Queue holds the prompts a front-end keeps for a session while its agent
// works. A queued prompt waits for the run to end, however long it goes on,
// and then runs as a prompt of its own. A forced prompt is handed to the
// running agent (Job.Steer), which reads it once the tool calls it is making
// finish; when the runner records it, it leaves the queue (Delivered). One
// the run ended before goes back to the head of the queue (Settle).
//
// A run that ends on its own lets the next prompt run; one that was stopped
// or failed pauses the queue, and nothing runs until the user resumes it.
// A Queue is not safe for concurrent use.
type Queue struct {
	// Items are forced prompts first, then queued ones in the order they
	// run.
	Items []QueueItem `json:"items"`
	// Paused keeps the queued prompts from running when a run ends.
	Paused bool `json:"paused,omitempty"`
}

// QueueItem is one prompt of a queue.
type QueueItem struct {
	// ID is the message ID the prompt runs or is delivered under.
	ID   string    `json:"id"`
	Text string    `json:"text"`
	At   time.Time `json:"at"`
	// Forced marks a prompt handed to the running agent.
	Forced bool `json:"forced,omitempty"`
	// Model is the model the prompt runs with, as it was chosen when the
	// prompt was queued; "" is the connection's. A forced prompt goes to the
	// running agent, whose model answers it.
	Model string `json:"model,omitempty"`
	// Images go with the prompt (see SetImages). Their bytes stay out of
	// the JSON, which describes them instead.
	Images []Image `json:"-"`
}

// Add queues a prompt, to run with model, at the end and returns it.
func (q *Queue) Add(text, model string) QueueItem {
	item := QueueItem{ID: uuid.New().String(), Text: text, At: time.Now().UTC(), Model: model}
	q.Items = append(q.Items, item)
	return item
}

// Insert puts a queued prompt back at index among the queued ones, as an
// edit that took it out leaves it.
func (q *Queue) Insert(index int, item QueueItem) {
	item.Forced = false
	at := q.forced() + max(0, min(index, len(q.Items)-q.forced()))
	q.Items = slices.Insert(q.Items, at, item)
}

// Get returns a prompt and its index among the queued ones, or among the
// forced ones for a forced prompt.
func (q *Queue) Get(id string) (QueueItem, int, bool) {
	i := q.index(id)
	if i < 0 {
		return QueueItem{}, -1, false
	}
	if q.Items[i].Forced {
		return q.Items[i], i, true
	}
	return q.Items[i], i - q.forced(), true
}

// Queued is how many prompts wait for the run to end.
func (q *Queue) Queued() int { return len(q.Items) - q.forced() }

// Forced is how many prompts the running agent has been handed.
func (q *Queue) Forced() int { return q.forced() }

// Remove takes a prompt out of the queue.
func (q *Queue) Remove(id string) (QueueItem, bool) {
	i := q.index(id)
	if i < 0 {
		return QueueItem{}, false
	}
	item := q.Items[i]
	q.Items = slices.Delete(q.Items, i, i+1)
	q.emptied()
	return item, true
}

// Edit replaces the text of a queued prompt. A forced prompt is the
// runner's already.
func (q *Queue) Edit(id, text string) bool {
	i := q.index(id)
	if i < 0 || q.Items[i].Forced {
		return false
	}
	q.Items[i].Text = text
	return true
}

// SetModel chooses the model a queued prompt runs with, "" for the
// connection's. A forced prompt is the runner's already.
func (q *Queue) SetModel(id, model string) bool {
	i := q.index(id)
	if i < 0 || q.Items[i].Forced {
		return false
	}
	q.Items[i].Model = model
	return true
}

// Move puts a queued prompt at index among the queued ones.
func (q *Queue) Move(id string, index int) bool {
	i := q.index(id)
	if i < 0 || q.Items[i].Forced {
		return false
	}
	item := q.Items[i]
	q.Items = slices.Delete(q.Items, i, i+1)
	q.Insert(index, item)
	return true
}

// Force marks a queued prompt as handed to the running agent, after the
// prompts handed to it before. Unforce undoes it when the runner cannot
// take it.
func (q *Queue) Force(id string) (QueueItem, bool) {
	i := q.index(id)
	if i < 0 {
		return QueueItem{}, false
	}
	item := q.Items[i]
	if item.Forced {
		return item, true
	}
	q.Items = slices.Delete(q.Items, i, i+1)
	item.Forced = true
	q.Items = slices.Insert(q.Items, q.forced(), item)
	return item, true
}

// Unforce puts a forced prompt the runner did not take back at the head of
// the queued ones.
func (q *Queue) Unforce(id string) {
	if i := q.index(id); i >= 0 && q.Items[i].Forced {
		item := q.Items[i]
		q.Items = slices.Delete(q.Items, i, i+1)
		q.Insert(0, item)
	}
}

// Delivered takes the forced prompts the runner recorded, which the
// transcript holds, out of the queue and returns them.
func (q *Queue) Delivered(tr *Transcript) []QueueItem {
	var delivered []QueueItem
	q.Items = slices.DeleteFunc(q.Items, func(item QueueItem) bool {
		if e := tr.Entry("input:" + item.ID); item.Forced && e != nil && e.State != Pending {
			delivered = append(delivered, item)
			return true
		}
		return false
	})
	if delivered != nil {
		q.emptied()
	}
	return delivered
}

// Settle takes the end of a run into account: forced prompts the agent never
// read go back to the head of the queue, in order, and a run that did not end
// on its own (outcome other than "done") pauses the queue.
func (q *Queue) Settle(outcome string) {
	for i := range q.Items {
		q.Items[i].Forced = false
	}
	if outcome != "done" && len(q.Items) != 0 {
		q.Paused = true
	}
}

// Next takes the prompt that runs next out of the queue, unless the queue is
// paused or holds forced prompts, which belong to a run that goes on.
func (q *Queue) Next() (QueueItem, bool) {
	if q.Paused || len(q.Items) == 0 || q.Items[0].Forced {
		return QueueItem{}, false
	}
	item := q.Items[0]
	q.Items = slices.Delete(q.Items, 0, 1)
	q.emptied()
	return item, true
}

// Clear empties the queue.
func (q *Queue) Clear() {
	q.Items, q.Paused = nil, false
}

// Clone copies the queue.
func (q *Queue) Clone() Queue {
	return Queue{Items: slices.Clone(q.Items), Paused: q.Paused}
}

func (q *Queue) index(id string) int {
	return slices.IndexFunc(q.Items, func(item QueueItem) bool { return item.ID == id })
}

func (q *Queue) forced() int {
	n := 0
	for n < len(q.Items) && q.Items[n].Forced {
		n++
	}
	return n
}

// emptied lifts the pause of a queue with nothing left in it: what is queued
// next waits for a run of its own.
func (q *Queue) emptied() {
	if len(q.Items) == 0 {
		q.Paused = false
	}
}
