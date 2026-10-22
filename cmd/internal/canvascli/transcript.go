package canvascli

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"net/url"
	"strconv"
	"strings"
	"time"
)

// transcriptCursor accepts the byte cursor returned by CanvasTranscript. The
// server also accepts numeric JSON for callers that naturally represent a
// cursor as a number, while "last" asks the server to use its per-reader
// cursor.
type transcriptCursor string

func (c *transcriptCursor) UnmarshalJSON(data []byte) error {
	data = bytes.TrimSpace(data)
	if len(data) == 0 {
		return errors.New("it is a cursor or last")
	}
	if data[0] == '"' {
		var value string
		if err := json.Unmarshal(data, &value); err != nil {
			return err
		}
		*c = transcriptCursor(value)
		return nil
	}
	value := jsontext.Value(data)
	if value.Kind() == 'n' {
		*c = transcriptCursor(string(data))
		return nil
	}
	return errors.New("it is a cursor or last")
}

type transcriptArgs struct {
	Node         string           `json:"node"`
	After        transcriptCursor `json:"after"`
	Limit        number           `json:"limit"`
	Tail         number           `json:"tail"`
	Kinds        nodeList         `json:"kinds"`
	Tools        nodeList         `json:"tools"`
	Since        string           `json:"since"`
	Until        string           `json:"until"`
	MessageLimit number           `json:"message_limit"`
	OutputLimit  number           `json:"output_limit"`
	Include      nodeList         `json:"include"`
	// Format is how the answer is given: text, or json as the server
	// gives it.
	Format string `json:"format"`
}

// transcriptPage is an answer of the server to a read of a transcript.
type transcriptPage struct {
	Node        string            `json:"node"`
	Session     string            `json:"session"`
	Cursor      string            `json:"cursor"`
	Next        string            `json:"next"`
	More        bool              `json:"more"`
	Tail        int               `json:"tail"`
	Older       int               `json:"older"`
	Reset       bool              `json:"cursor_reset"`
	ResetReason string            `json:"cursor_reset_reason"`
	Entries     []transcriptEntry `json:"entries"`
	Notes       *includedText     `json:"notes"`
	Summaries   *includedText     `json:"summaries"`
}

type transcriptEntry struct {
	Kind   string          `json:"kind"`
	At     time.Time       `json:"at"`
	Text   string          `json:"text"`
	Detail string          `json:"detail"`
	Phase  string          `json:"phase"`
	State  string          `json:"state"`
	Forced bool            `json:"forced"`
	Tool   *transcriptTool `json:"tool"`
}

type transcriptTool struct {
	Name       string    `json:"name"`
	Input      string    `json:"input"`
	State      string    `json:"state"`
	ExitCode   *int      `json:"exit_code"`
	Output     string    `json:"output"`
	Stderr     string    `json:"stderr"`
	Error      string    `json:"error"`
	Started    time.Time `json:"started"`
	Finished   time.Time `json:"finished"`
	FullOutput *fileRef  `json:"full_output"`
	FullStderr *fileRef  `json:"full_stderr"`
}

type fileRef struct {
	Path string `json:"path"`
	Size int64  `json:"size"`
}

type includedText struct {
	File fileRef `json:"file"`
	Text string  `json:"text"`
}

type searchArgs struct {
	Node  string `json:"node"`
	Nodes string `json:"nodes"`
	Query string `json:"q"`
	Limit number `json:"limit"`
}

func (c *client) transcript(ctx context.Context, a transcriptArgs) (result, error) {
	node := strings.TrimSpace(a.Node)
	if node == "" {
		return result{}, errors.New("name the node whose transcript to read")
	}
	format := strings.ToLower(strings.TrimSpace(a.Format))
	if format != "" && format != "text" && format != "json" {
		return result{}, errors.New("format is text or json")
	}
	q := url.Values{}
	if after := strings.TrimSpace(string(a.After)); after != "" {
		q.Set("after", after)
	}
	if value := intOf(a.Limit); value != nil {
		q.Set("limit", strconv.Itoa(*value))
	}
	if value := intOf(a.Tail); value != nil {
		q.Set("tail", strconv.Itoa(*value))
	}
	if values := paths(a.Kinds); len(values) != 0 {
		q.Set("kinds", strings.Join(values, ","))
	}
	if values := paths(a.Tools); len(values) != 0 {
		q.Set("tools", strings.Join(values, ","))
	}
	if value := strings.TrimSpace(a.Since); value != "" {
		q.Set("since", value)
	}
	if value := strings.TrimSpace(a.Until); value != "" {
		q.Set("until", value)
	}
	if value := intOf(a.MessageLimit); value != nil {
		q.Set("message_limit", strconv.Itoa(*value))
	}
	if value := intOf(a.OutputLimit); value != nil {
		q.Set("output_limit", strconv.Itoa(*value))
	}
	if values := paths(a.Include); len(values) != 0 {
		q.Set("include", strings.Join(values, ","))
	}
	path := "/api/canvas/nodes/" + url.PathEscape(node) + "/transcript"
	var page transcriptPage
	raw, err := c.call(ctx, "GET", path, q, nil, &page)
	if err != nil {
		return result{}, err
	}
	if format == "json" {
		// Compact: the indentation would take a tenth of what a reader
		// reads.
		return result{text: string(bytes.TrimSpace(raw)), raw: raw}, nil
	}
	return result{text: formatTranscript(page), raw: raw}, nil
}

// formatTranscript is a page of a transcript as text: a line or a block an
// entry, at its local time, and how to read on.
func formatTranscript(p transcriptPage) string {
	var b strings.Builder
	fmt.Fprintf(&b, "transcript of %s (session %s): %d entries", p.Node, p.Session, len(p.Entries))
	switch {
	case p.Tail > 0:
		fmt.Fprintf(&b, ", the last that match; %d older before them", p.Older)
	case p.Cursor != "":
		fmt.Fprintf(&b, " after %s", p.Cursor)
	}
	fmt.Fprintf(&b, "; next %s\n", p.Next)
	if p.Reset {
		reason := p.ResetReason
		if reason == "" {
			reason = "the cursor was not valid"
		}
		fmt.Fprintf(&b, "cursor reset: %s\n", reason)
	}
	day := ""
	for _, e := range p.Entries {
		at := e.At.Local()
		if !e.At.IsZero() {
			if d := at.Format("2006-01-02 MST"); d != day {
				day = d
				fmt.Fprintf(&b, "── %s ──\n", d)
			}
		}
		clock := "--:--:--"
		if !e.At.IsZero() {
			clock = at.Format("15:04:05")
		}
		label := e.Kind
		if e.Phase != "" {
			label += "/" + e.Phase
		}
		var marks []string
		if e.Forced {
			marks = append(marks, "mid-turn")
		}
		if e.State != "" {
			marks = append(marks, e.State)
		}
		if t := e.Tool; t != nil {
			label = "tool " + t.Name
			if t.State != "" {
				marks = append(marks, t.State)
			}
			if t.ExitCode != nil {
				marks = append(marks, fmt.Sprintf("exit %d", *t.ExitCode))
			}
			if !t.Started.IsZero() && !t.Finished.IsZero() {
				marks = append(marks, t.Finished.Sub(t.Started).Round(100*time.Millisecond).String())
			}
		}
		if len(marks) != 0 {
			label += " [" + strings.Join(marks, ", ") + "]"
		}
		text := e.Text
		if e.Tool != nil {
			text = e.Tool.Input
		}
		fmt.Fprintf(&b, "%s %s:", clock, label)
		if strings.TrimSpace(text) == "" {
			b.WriteString("\n")
		} else {
			writeIndented(&b, " ", "    ", text)
		}
		if e.Detail != "" {
			writeIndented(&b, "    detail: ", "    ", e.Detail)
		}
		if t := e.Tool; t != nil {
			for _, part := range []struct{ name, text string }{{"output", t.Output}, {"stderr", t.Stderr}, {"error", t.Error}} {
				if strings.TrimSpace(part.text) != "" {
					writeIndented(&b, "  "+part.name+":\n    │ ", "    │ ", part.text)
				}
			}
			for _, file := range []struct {
				name string
				ref  *fileRef
			}{{"full output", t.FullOutput}, {"full stderr", t.FullStderr}} {
				if file.ref != nil {
					fmt.Fprintf(&b, "  %s: %s (%d bytes)\n", file.name, file.ref.Path, file.ref.Size)
				}
			}
		}
	}
	if p.More {
		fmt.Fprintf(&b, "more: entries after these did not fit this answer; read on with after %q\n", p.Next)
	}
	for _, part := range []struct {
		name string
		text *includedText
	}{{"notes", p.Notes}, {"summaries", p.Summaries}} {
		if part.text != nil {
			fmt.Fprintf(&b, "%s (%s, %d bytes):\n", part.name, part.text.File.Path, part.text.File.Size)
			writeIndented(&b, "  ", "  ", part.text.Text)
		}
	}
	return b.String()
}

// writeIndented writes text after first, its other lines after indent, and
// ends it with a newline.
func writeIndented(b *strings.Builder, first, indent, text string) {
	text = strings.TrimRight(text, "\n")
	for index, line := range strings.Split(text, "\n") {
		if index == 0 {
			b.WriteString(first)
		} else {
			b.WriteString("\n" + indent)
		}
		b.WriteString(line)
	}
	b.WriteString("\n")
}

func (c *client) search(ctx context.Context, a searchArgs) (result, error) {
	node := strings.TrimSpace(a.Node)
	if node == "" {
		node = "self"
	}
	query := strings.TrimSpace(a.Query)
	if query == "" {
		return result{}, errors.New("name a search query with q")
	}
	q := url.Values{"q": []string{query}}
	if nodes := strings.TrimSpace(a.Nodes); nodes != "" {
		q.Set("nodes", nodes)
	}
	if value := intOf(a.Limit); value != nil {
		q.Set("limit", strconv.Itoa(*value))
	}
	path := "/api/canvas/nodes/" + url.PathEscape(node) + "/search"
	raw, err := c.call(ctx, "GET", path, q, nil, &map[string]any{})
	if err != nil {
		return result{}, err
	}
	return result{text: prettyJSON(raw), raw: raw}, nil
}

func prettyJSON(data []byte) string {
	value := jsontext.Value(bytes.TrimSpace(data)).Clone()
	if err := value.Indent(jsontext.WithIndentPrefix(""), jsontext.WithIndent("  ")); err == nil {
		return string(value)
	}
	return string(data)
}
