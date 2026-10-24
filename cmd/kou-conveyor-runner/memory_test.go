package main

import (
	"context"
	"errors"
	"runtime/debug"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/agentrunner"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

type answering struct{ requests int }

func (c *answering) Respond(context.Context, llm.Request, llm.RequestOptions) (llm.Response, error) {
	c.requests++
	return llm.Response{Stop: llm.StopComplete}, nil
}

func (c *answering) Close() error { return nil }

func percent() int {
	p := debug.SetGCPercent(-1)
	debug.SetGCPercent(p)
	return p
}

func providing(client agentrunner.Client, err error) []agentrunner.Provider {
	return []agentrunner.Provider{{
		Name: "fake",
		NewClient: func(string, string, int, func(string) string) (agentrunner.Client, error) {
			return client, err
		},
	}}
}

func TestTuneMemoryCollectsOftenUntilTheFirstRequest(t *testing.T) {
	defer debug.SetGCPercent(debug.SetGCPercent(100))
	inner := &answering{}
	providers := tuneMemory(func(string) string { return "" }, providing(inner, nil))
	if got := percent(); got != readingGCPercent {
		t.Fatalf("GC percent while reading = %d, want %d", got, readingGCPercent)
	}
	client, err := providers[0].NewClient("", "", 1, nil)
	if err != nil {
		t.Fatal(err)
	}
	if got := percent(); got != readingGCPercent {
		t.Fatalf("GC percent before the first request = %d, want %d", got, readingGCPercent)
	}
	for range 2 {
		if _, err := client.Respond(t.Context(), llm.Request{}, llm.RequestOptions{}); err != nil {
			t.Fatal(err)
		}
		if got := percent(); got != 100 {
			t.Fatalf("GC percent after the first request = %d, want 100", got)
		}
	}
	if inner.requests != 2 {
		t.Fatalf("requests reaching the client = %d, want 2", inner.requests)
	}
	if err := client.Close(); err != nil {
		t.Fatal(err)
	}
}

func TestTuneMemoryLeavesTheUsersSettings(t *testing.T) {
	defer debug.SetGCPercent(debug.SetGCPercent(100))
	for _, name := range []string{"GOGC", "GOMEMLIMIT"} {
		inner := &answering{}
		providers := tuneMemory(func(key string) string {
			if key == name {
				return "50"
			}
			return ""
		}, providing(inner, nil))
		if got := percent(); got != 100 {
			t.Fatalf("with %s set: GC percent = %d, want it untouched (100)", name, got)
		}
		client, err := providers[0].NewClient("", "", 1, nil)
		if err != nil {
			t.Fatal(err)
		}
		if client != agentrunner.Client(inner) {
			t.Fatalf("with %s set: the client is wrapped", name)
		}
	}
}

func TestTuneMemoryPassesAClientsError(t *testing.T) {
	defer debug.SetGCPercent(debug.SetGCPercent(100))
	failure := errors.New("no key")
	providers := tuneMemory(func(string) string { return "" }, providing(nil, failure))
	if _, err := providers[0].NewClient("", "", 1, nil); !errors.Is(err, failure) {
		t.Fatalf("error = %v, want %v", err, failure)
	}
}
