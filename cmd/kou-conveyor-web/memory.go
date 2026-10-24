package main

import "runtime/debug"

// The server keeps the transcripts of the sessions the pages show, and
// makes one anew, whole, when a session is opened that it does not keep: a
// burst of hundreds of megabytes for a long session. The heap grows to hold
// the burst, and on macOS what it grew to stays the server's: the runtime
// gives the pages back, but the system goes on counting them once the
// server has started a process since, which it does every few seconds (git,
// runners, terminals). So the collector runs when the heap has grown by
// half since the last collection rather than doubled: opening four long
// sessions peaks about a quarter lower, for about a fifth more of the CPU
// the opening takes. A GOGC or GOMEMLIMIT of the user's own is left as it
// is.

// serverGCPercent is the collector's target in the server.
const serverGCPercent = 50

// tuneMemory sets the collector for the server.
func tuneMemory(getenv func(string) string) {
	if getenv("GOGC") != "" || getenv("GOMEMLIMIT") != "" {
		return
	}
	debug.SetGCPercent(serverGCPercent)
}
