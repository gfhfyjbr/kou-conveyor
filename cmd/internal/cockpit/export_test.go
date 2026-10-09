package cockpit

// ForgetRunners has the next run ask its runner again what it can do.
func ForgetRunners() {
	runnerSteering.Lock()
	clear(runnerSteering.byBinary)
	runnerSteering.Unlock()
}
