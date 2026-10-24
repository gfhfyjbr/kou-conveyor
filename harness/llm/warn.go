package llm

import "context"

type warningReporter struct{}

// WithWarningReporter returns a context whose model requests call report with
// a warning about something an adapter dealt with on its own instead of
// failing the request, such as images it left out because the model cannot
// read them.
func WithWarningReporter(ctx context.Context, report func(string)) context.Context {
	return context.WithValue(ctx, warningReporter{}, report)
}

// ReportWarning tells the context's reporter, if there is one, of a warning.
func ReportWarning(ctx context.Context, message string) {
	if report, ok := ctx.Value(warningReporter{}).(func(string)); ok && report != nil {
		report(message)
	}
}
