//go:build unix

package agentrunner

import "syscall"

// keepLock keeps file descriptor 3, the session's lock that a front-end
// hands a detached runner, from the commands the run starts.
func keepLock() { syscall.CloseOnExec(3) }
