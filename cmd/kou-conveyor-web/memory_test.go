package main

import (
	"runtime/debug"
	"testing"
)

func TestTuneMemory(t *testing.T) {
	previous := debug.SetGCPercent(100)
	defer debug.SetGCPercent(previous)
	tuneMemory(func(string) string { return "" })
	if got := debug.SetGCPercent(100); got != serverGCPercent {
		t.Fatalf("GC percent %d, want %d", got, serverGCPercent)
	}
	for _, name := range []string{"GOGC", "GOMEMLIMIT"} {
		tuneMemory(func(key string) string {
			if key == name {
				return "200"
			}
			return ""
		})
		if got := debug.SetGCPercent(100); got != 100 {
			t.Fatalf("with %s set, GC percent %d, want the user's", name, got)
		}
	}
}
