package canvas

import "testing"

func TestCloseLeavesAgentRunsToTheirHost(t *testing.T) {
	e := newEngine(t)
	id, c := e.newCanvas(t, "shutdown")
	agent := e.ops(t, id, `[{"op":"node.add","node":{"kind":"agent","title":"worker"}}]`).IDs["0"]
	session := sessionOf(t, c, agent)
	e.host.mu.Lock()
	e.host.active[session] = true
	e.host.mu.Unlock()
	e.Close()
	e.host.mu.Lock()
	defer e.host.mu.Unlock()
	if !e.host.active[session] {
		t.Fatal("closing the canvas released the host's active run")
	}
}
