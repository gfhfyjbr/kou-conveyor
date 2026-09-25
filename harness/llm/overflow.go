package llm

// ContextOverflowError reports a request the model's context window cannot
// hold, input and room for the response together. Tokens is the size of the
// request and Limit the size of the window, as the provider counted them;
// either is 0 when the provider did not say.
type ContextOverflowError struct {
	Tokens, Limit int64
	Err           error
}

func (err *ContextOverflowError) Error() string { return err.Err.Error() }

func (err *ContextOverflowError) Unwrap() error { return err.Err }
