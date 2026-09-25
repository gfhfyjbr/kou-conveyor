package operation

func prepareRemoteJobOutput(current Operation, state *RemoteJobState) {
	// TODO: Capture the full result in a file before truncating and expose its path.
	limit := current.MaxOutputLength
	if !state.ResultTruncated || state.TerminalResult == "" {
		state.ResultBytes = len(state.TerminalResult)
		state.TerminalResult, state.ResultTruncated = BoundOutput(state.TerminalResult, limit)
	}
	if !state.ErrorTruncated || state.TerminalError == "" {
		state.ErrorBytes = len(state.TerminalError)
		state.TerminalError, state.ErrorTruncated = BoundOutput(state.TerminalError, limit)
	}
}
