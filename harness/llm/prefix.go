package llm

import (
	"context"
	"encoding/json/jsontext"
)

// Prefix fingerprints what a request to the model began with, as the
// adapter sent it: its tools, each block of its system prompt, and the
// messages up to each one. A provider caches a request's prefix and serves
// the next request from it only as far as the two are the same, so the
// first fingerprint two requests differ in is where the cache of the one
// before stopped serving the next — without the requests themselves, which
// are megabytes.
//
// The fingerprints are of the request as the adapter sent it, before a
// gateway in between changes it; markers that ask for a cache are left out
// of them, as they are no part of what is cached, and kept in Breakpoints.
type Prefix struct {
	Tools  string   `json:"tools,omitempty"`
	System []string `json:"system,omitempty"`
	// Messages holds, for each message, the fingerprint of the messages up
	// to it and with it: the request's messages are the same as another's
	// up to the last index where the two hold the same fingerprint.
	Messages []string `json:"messages,omitempty"`
	// Breakpoints are where the request asked for what comes before to be
	// cached.
	Breakpoints []Breakpoint `json:"breakpoints,omitempty"`
	// Edits are the context edits the provider makes before the model reads
	// the request, such as clearing older thinking, which change what can
	// be served from the cache.
	Edits jsontext.Value `json:"edits,omitzero"`
}

// Breakpoint is a place in a request that asks for what comes before it to
// be cached.
type Breakpoint struct {
	// Message is the index of the message the breakpoint ends, or -1 for
	// the end of the system prompt.
	Message int `json:"message"`
	// TTL is how long the cache is to be kept, as the provider spells it
	// ("5m", "1h").
	TTL string `json:"ttl"`
}

// PrefixReport is the prefix of a request a response answered.
type PrefixReport struct {
	Model    string // the model as the request named it
	Response string // the response's ID
	Usage    Usage
	Prefix   Prefix
}

type prefixReporter struct{}

// WithPrefixReporter returns a context whose model requests call report
// with their prefix once a response answers them, for adapters that take
// fingerprints of what they send.
func WithPrefixReporter(ctx context.Context, report func(PrefixReport)) context.Context {
	return context.WithValue(ctx, prefixReporter{}, report)
}

// ReportPrefix tells the context's reporter, if there is one, what the
// request a response answered began with. The report is made only for a
// reporter: fingerprints take reading the whole request again.
func ReportPrefix(ctx context.Context, report func() PrefixReport) {
	if reporter, ok := ctx.Value(prefixReporter{}).(func(PrefixReport)); ok && reporter != nil {
		reporter(report())
	}
}
