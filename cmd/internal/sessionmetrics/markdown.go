package sessionmetrics

import (
	"fmt"
	"io"
	"strings"
	"time"
)

// Markdown writes the report under the heading title, as markdown tables;
// list adds the requests that rewrote the cache or started cold.
func (r Report) Markdown(w io.Writer, title string, list bool) error {
	var b strings.Builder
	fmt.Fprintf(&b, "## %s\n\n", title)
	if r.Session != "" {
		fmt.Fprintf(&b, "Session %s", r.Session)
	} else {
		b.WriteString("A session")
	}
	if !r.First.IsZero() {
		fmt.Fprintf(&b, ", %s – %s UTC", stamp(r.First), stamp(r.Last))
	}
	if !r.Since.IsZero() {
		fmt.Fprintf(&b, " (since %s UTC)", stamp(r.Since))
	}
	b.WriteString(".\n\n")

	b.WriteString("| What | Count | Cost | Share |\n|---|---:|---:|---:|\n")
	row := func(what string, count int, cost, share float64) {
		fmt.Fprintf(&b, "| %s | %d | %s | %s |\n", what, count, amount(cost), percent(share))
	}
	row("Requests", r.Requests, r.Cost, 1)
	row("Full cache rewrites", r.Rewrites.Count, r.Rewrites.Cost, r.Rewrites.Share)
	for _, part := range r.Rewrites.Causes {
		row("· "+part.Name, part.Count, part.Cost, part.Share)
	}
	row("Requests that only wait", r.Waits.Requests, r.Waits.Cost, r.Waits.Share)
	row("Cold starts", r.ColdStarts.Count, r.ColdStarts.Cost, r.ColdStarts.Share)
	for _, part := range r.ColdStarts.Parts {
		row("· "+part.Name, part.Count, part.Cost, part.Share)
	}
	b.WriteString("\n")
	fmt.Fprintf(&b, "Turns %d, API turns %d. Tokens: input %s, read from the cache %s, written to it %s, output %s.",
		r.Turns, r.APITurns, amount(float64(r.Tokens.Input)), amount(float64(r.Tokens.Read)),
		amount(float64(r.Tokens.Written)), amount(float64(r.Tokens.Output)))
	if len(r.Rewrites.Also) > 0 {
		b.WriteString(" Rewrites with each cause, whatever came first:")
		for i, name := range []string{"new run", "thinking window", "prune"} {
			if i > 0 {
				b.WriteString(",")
			}
			fmt.Fprintf(&b, " %s %d", name, r.Rewrites.Also[name])
		}
		b.WriteString(".")
	}
	fmt.Fprintf(&b, " Waiting tool calls: %d.\n\n", r.Waits.Calls)

	b.WriteString("| Event | Count | Detail |\n|---|---:|---|\n")
	m := r.Messages
	fmt.Fprintf(&b, "| Messages | %d | %d after tool calls, %d opened a run, %d from the canvas (%d replies) |\n",
		m.Count, m.AfterTools, m.OpenedRun, m.FromCanvas, m.Replies)
	c := r.Compactions
	fmt.Fprintf(&b, "| Compactions | %d | %d requests; message kept %d, notes clipped in it %d; asks to shorten the notes %d |\n",
		c.Count, c.Requests, c.Kept, c.NotesClipped, c.OverflowAsks)
	total, kinds := 0, []string{}
	for _, kind := range []string{"progress report", "compaction soon", "after compaction", "notes changed", "stopped before", "waited for tools", "output limit", "refused", "other"} {
		if n := r.Heartbeats[kind]; n > 0 {
			total += n
			kinds = append(kinds, fmt.Sprintf("%s %d", kind, n))
		}
	}
	fmt.Fprintf(&b, "| Heartbeats | %d | %s |\n", total, strings.Join(kinds, ", "))
	fmt.Fprintf(&b, "| Stops | %d | |\n", len(r.Stops))
	fmt.Fprintf(&b, "| Tool calls that failed to start | %d | %d with arguments that could not be decoded |\n",
		r.ToolErrors.Count, r.ToolErrors.Arguments)
	if r.Unreadable > 0 {
		fmt.Fprintf(&b, "| Unreadable lines | %d | %s |\n", r.Unreadable, cell(r.Problem))
	}
	if len(r.Stops) > 0 {
		b.WriteString("\nStops:\n")
		for _, stop := range r.Stops {
			fmt.Fprintf(&b, "- %s UTC: %s\n", stamp(stop.At), strings.TrimSpace(stop.Reason))
		}
	}
	if list {
		details(&b, "Full cache rewrites", r.Rewrites.List)
		details(&b, "Cold starts", r.ColdStarts.List)
	}
	b.WriteString("\n")
	_, err := io.WriteString(w, b.String())
	return err
}

func details(b *strings.Builder, title string, list []RewriteDetail) {
	if len(list) == 0 {
		return
	}
	fmt.Fprintf(b, "\n%s:\n\n| At (UTC) | Input | Read | Written | Pause, min | After a compaction | Causes |\n|---|---:|---:|---:|---:|---|---|\n", title)
	for _, d := range list {
		compacted := ""
		if d.Compacted {
			compacted = "yes"
		}
		fmt.Fprintf(b, "| %s | %d | %d | %d | %.1f | %s | %s |\n",
			stamp(d.At), d.Input, d.Read, d.Written, float64(d.Pause), compacted, strings.Join(d.Causes, ", "))
	}
}

func stamp(t time.Time) string { return t.UTC().Format("2006-01-02 15:04:05") }

func amount(v float64) string {
	switch {
	case v >= 1e6:
		return fmt.Sprintf("%.2fM", v/1e6)
	case v >= 1e3:
		return fmt.Sprintf("%.0fk", v/1e3)
	}
	return fmt.Sprintf("%.0f", v)
}

func percent(share float64) string { return fmt.Sprintf("%.1f%%", share*100) }

// cell makes text fit a table cell.
func cell(text string) string {
	return strings.NewReplacer("|", "\\|", "\n", " ").Replace(text)
}
