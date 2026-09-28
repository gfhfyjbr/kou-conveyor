package highlight

import (
	"container/list"
	"context"
	"sync"
	"time"
)

// Cache keeps highlighted texts by a key that changes with the file — its
// path, size and time — and highlights large ones in the background: the
// first to ask gets what a short deadline allows, and whoever asks for the
// rest waits for the one highlighting it.
type Cache struct {
	// limit is how many runs it keeps at most, over all its texts.
	limit int

	mu    sync.Mutex
	order *list.List // of *entry, the most recently used first
	byKey map[string]*list.Element
	runs  int
	jobs  map[string]*job
}

type entry struct {
	key    string
	result Result
}

type job struct {
	done   chan struct{}
	result Result
}

// NewCache returns a cache that keeps up to limit runs.
func NewCache(limit int) *Cache {
	return &Cache{limit: limit, order: list.New(), byKey: make(map[string]*list.Element), jobs: make(map[string]*job)}
}

// Get returns the text highlighted as far as budget allows, or all of it
// if it was highlighted before; when it is not complete, the highlighting
// goes on in the background, for Wait.
func (c *Cache) Get(key, name, text string, budget time.Duration) Result {
	if result, ok := c.lookup(key); ok {
		return result
	}
	result := Highlight(name, text, time.Now().Add(budget))
	if result.Complete {
		c.store(key, result)
		return result
	}
	c.start(key, name, text)
	return result
}

// Wait returns the text highlighted in full, waiting for the background
// work Get left, or doing it.
func (c *Cache) Wait(ctx context.Context, key, name, text string) (Result, error) {
	if result, ok := c.lookup(key); ok {
		return result, nil
	}
	j := c.start(key, name, text)
	select {
	case <-j.done:
		return j.result, nil
	case <-ctx.Done():
		return Result{}, ctx.Err()
	}
}

// start highlights a text in the background, once for a key.
func (c *Cache) start(key, name, text string) *job {
	c.mu.Lock()
	defer c.mu.Unlock()
	if j := c.jobs[key]; j != nil {
		return j
	}
	j := &job{done: make(chan struct{})}
	c.jobs[key] = j
	go func() {
		// A lexer that goes badly astray gives up rather than hold a core.
		result := Highlight(name, text, time.Now().Add(time.Minute))
		result.Complete = true
		j.result = result
		c.store(key, result)
		c.mu.Lock()
		delete(c.jobs, key)
		c.mu.Unlock()
		close(j.done)
	}()
	return j
}

func (c *Cache) lookup(key string) (Result, bool) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if element := c.byKey[key]; element != nil {
		c.order.MoveToFront(element)
		return element.Value.(*entry).result, true
	}
	return Result{}, false
}

func (c *Cache) store(key string, result Result) {
	c.mu.Lock()
	defer c.mu.Unlock()
	if element := c.byKey[key]; element != nil {
		c.runs -= len(element.Value.(*entry).result.Runs)
		c.order.Remove(element)
	}
	if len(result.Runs) > c.limit {
		return
	}
	c.byKey[key] = c.order.PushFront(&entry{key: key, result: result})
	c.runs += len(result.Runs)
	for c.runs > c.limit {
		oldest := c.order.Back()
		e := oldest.Value.(*entry)
		c.order.Remove(oldest)
		delete(c.byKey, e.key)
		c.runs -= len(e.result.Runs)
	}
}
