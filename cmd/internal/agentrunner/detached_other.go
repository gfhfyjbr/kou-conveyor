//go:build !unix

package agentrunner

// keepLock has nothing to keep: front-ends detach runners only where
// sessions are locked with flock.
func keepLock() {}
