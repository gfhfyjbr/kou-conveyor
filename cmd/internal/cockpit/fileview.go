package cockpit

import (
	"fmt"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"time"

	"github.com/aymanbagabas/go-udiff"
	"github.com/aymanbagabas/go-udiff/lcs"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/highlight"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
)

// FileView is what a Write call wrote or an Edit call changed, as the
// cockpit shows it: the code in the colours of its tokens, by the file's
// name.
type FileView struct {
	// Kind is the call's: "write" or "edit".
	Kind string `json:"kind"`
	// Content is what a Write call wrote, whole.
	Content string `json:"content,omitempty"`
	// Diff is what an Edit call changed, as the hunks of a unified diff,
	// without the heads of the files: the lines the string it replaced was
	// in, those it put in their place, and around them the lines of the
	// file, as the edit's result shows them — or, where it does not, the
	// strings' own lines. A line is a hunk's head ("@@ -12,3 +12,4 @@"), or
	// a sign ("-", "+" or " ") and a line of code; a newline ends each line
	// but the last.
	Diff string `json:"diff,omitempty"`
	// Line is the line of the file the change starts at, as the edit said,
	// and the hunks number the file's lines; 0 when it did not say (it
	// failed), and the hunks number the strings'.
	Line int `json:"line,omitzero"`
	// Syntax highlights Content, or Diff, the code in its lines in the
	// colours it has in the string it is of — signs and hunks' heads are
	// plain — as Tool.Syntax does a Code call's code.
	Syntax []int32 `json:"syntax,omitempty"`
	// Replacements is how many times an Edit call made its change, when it
	// made it more than once (replace_all): Line is where the first is.
	Replacements int `json:"replacements,omitzero"`
}

// fileOp is what the operation of a Write or an Edit call does: the file,
// and what the call wrote there, or the string it replaced, where and how
// many times, and what the edit said of it. The zero fileOp does neither.
type fileOp struct {
	kind, path, content, old, replacement, said string
	line, replacements                          int
}

// fileOpOf reads what a file operation writes or edits.
func fileOpOf(state operation.FileState) fileOp {
	switch state.Action {
	case operation.FileWrite:
		return fileOp{kind: "write", path: state.Path, content: Clean(state.Content)}
	case operation.FileEdit:
		op := fileOp{kind: "edit", path: state.Path, old: Clean(state.OldString), replacement: Clean(state.NewString)}
		if state.Result != nil {
			op.said = Clean(state.Result.Text)
			op.line, op.replacements = editPlace(op.said)
		}
		return op
	}
	return fileOp{}
}

// editSaid is what an edit says of where it changed its file, and how many
// times (operation.RunFile): "replaced 1 occurrence at line 43", "replaced
// 4 occurrences, the first at line 43".
var editSaid = regexp.MustCompile(`replaced (\d+) occurrences?(?:, the first)? at line (\d+)`)

// editPlace reads the line an edit changed its file at, and how many times
// it did, from the first line of what it said: zeros where it does not say.
// The path comes first there, so the last match is the edit's.
func editPlace(text string) (line, replacements int) {
	first, _, _ := strings.Cut(text, "\n")
	matches := editSaid.FindAllStringSubmatch(first, -1)
	if len(matches) == 0 {
		return 0, 0
	}
	m := matches[len(matches)-1]
	replacements, _ = strconv.Atoi(m[1])
	line, _ = strconv.Atoi(m[2])
	return line, replacements
}

// view draws what the operation does, as FileView shows it, highlighting it
// for at most budget.
func (op fileOp) view(budget time.Duration) *FileView {
	deadline := time.Now().Add(budget)
	if op.kind == "write" {
		return &FileView{Kind: op.kind, Content: op.content, Syntax: highlight.Remembered(op.path, op.content, deadline).Runs}
	}
	view := &FileView{Kind: op.kind, Line: op.line}
	before, after, from := op.old, op.replacement, op.line
	if b, a, f, ok := inFile(op.said, op.old, op.replacement, op.line, op.replacements); ok {
		before, after, from = b, a, f
	}
	view.Diff, view.Syntax = editDiff(op.path, before, after, from, deadline)
	if op.replacements > 1 {
		view.Replacements = op.replacements
	}
	return view
}

// inFile puts an edit in the lines of its file: those it changed, whole,
// and those around them, as what the edit said shows the file after it
// (fileops' snippet: the lines around the change, numbered "%6d\t%s"). It
// returns the text of those lines before the edit and after, and the line
// of the file they start at. ok is false where what the edit said does not
// tell: where in the line it starts the replacement is (the string may be
// there more than once, or be empty), or what lines after the first change
// a replace_all changed too.
func inFile(said, old, replacement string, line, replacements int) (before, after string, from int, ok bool) {
	_, snippet, found := strings.Cut(said, "\n")
	if !found || line < 1 || replacements > 1 {
		return "", "", 0, false
	}
	// The lines, each with the newline that ends it: the file's last has
	// one, if the replacement ends it with one, the snippet not showing the
	// empty line after it.
	var text strings.Builder
	start, end := -1, -1 // where the line the replacement starts in is
	for i, numbered := range strings.Split(snippet, "\n") {
		number, code, found := strings.Cut(numbered, "\t")
		n, err := strconv.Atoi(strings.TrimSpace(number))
		if !found || err != nil || i > 0 && n != from+i {
			return "", "", 0, false
		}
		if i == 0 {
			from = n
		}
		if n == line {
			start, end = text.Len(), text.Len()+len(code)
		}
		text.WriteString(code + "\n")
	}
	if start < 0 {
		return "", "", 0, false
	}
	// The replacement starts in that line, and only one place in it may be
	// where: the lines before the edit had the string it replaced there.
	after = text.String()
	at := -1
	for i := start; i <= end; i++ {
		j := strings.Index(after[i:], replacement)
		if j < 0 || i+j > end {
			break
		}
		if at >= 0 {
			return "", "", 0, false
		}
		at, i = i+j, i+j
	}
	if at < 0 {
		return "", "", 0, false
	}
	return after[:at] + old + after[at+len(replacement):], after, from, true
}

// editDiff is the diff of an edit of the file name that made replacement
// of old, text of the file from its line line on (0 if it is not known: the
// diff numbers the text's own lines), and the highlighting of its code, as
// the file's name says, until deadline.
func editDiff(name, old, replacement string, line int, deadline time.Time) (string, []int32) {
	if old == replacement {
		return "", nil
	}
	// The strings are pieces of the file: that the last lines of both end
	// where they do is no change.
	if !strings.HasSuffix(old, "\n") && !strings.HasSuffix(replacement, "\n") {
		old, replacement = old+"\n", replacement+"\n"
	}
	diff, err := udiff.ToUnifiedDiff("", "", old, lineEdits(old, replacement), udiff.DefaultContextLines)
	if err != nil || len(diff.Hunks) == 0 {
		return "", nil
	}
	diff.Hunks = joinHunks(diff.Hunks, old)
	// Where a hunk is in replacement, go-udiff (v0.3.1) counts wrong past a
	// hunk that joins two edits — it leaves out the lines between them: it
	// is where the hunk is in old, moved by the lines the hunks before it
	// put in and took out.
	delta := 0
	for _, hunk := range diff.Hunks {
		hunk.ToLine = hunk.FromLine + delta
		for _, l := range hunk.Lines {
			switch l.Kind {
			case udiff.Insert:
				delta++
			case udiff.Delete:
				delta--
			}
		}
	}
	before := highlight.Lines(old, highlight.Remembered(name, old, deadline).Runs)
	after := highlight.Lines(replacement, highlight.Remembered(name, replacement, deadline).Runs)
	shift := max(line-1, 0)
	var text strings.Builder
	var runs []int32
	// put adds a line to the diff: plain, a sign or a hunk's head, then
	// code, in the colours its tokens have in the string it is a line of,
	// the line at index of lines.
	put := func(plain, code string, lines [][]int32, index int) {
		if text.Len() > 0 {
			text.WriteByte('\n')
			runs = highlight.Append(runs, 1, 0)
		}
		code = strings.TrimSuffix(code, "\n")
		text.WriteString(plain)
		text.WriteString(code)
		runs = highlight.Append(runs, highlight.Len(plain), 0)
		// The line's runs cover it whole; were they to fall short or run
		// over, the diff's runs would still keep to its text.
		width := highlight.Len(code)
		if index >= 0 && index < len(lines) {
			for i := 0; i+1 < len(lines[index]) && width > 0; i += 2 {
				n := min(lines[index][i], width)
				runs = highlight.Append(runs, n, lines[index][i+1])
				width -= n
			}
		}
		runs = highlight.Append(runs, width, 0)
	}
	for _, hunk := range diff.Hunks {
		removed, added := 0, 0
		for _, l := range hunk.Lines {
			if l.Kind != udiff.Insert {
				removed++
			}
			if l.Kind != udiff.Delete {
				added++
			}
		}
		put(fmt.Sprintf("@@ -%d,%d +%d,%d @@", hunk.FromLine+shift, removed, hunk.ToLine+shift, added), "", nil, -1)
		from, to := hunk.FromLine-1, hunk.ToLine-1
		for _, l := range hunk.Lines {
			switch l.Kind {
			case udiff.Delete:
				put("-", l.Content, before, from)
				from++
			case udiff.Insert:
				put("+", l.Content, after, to)
				to++
			default:
				put(" ", l.Content, before, from)
				from, to = from+1, to+1
			}
		}
	}
	return text.String(), runs
}

// joinHunks joins the hunks of a diff of old that no more lines of it than
// a hunk's context keep apart: those lines show as context, not as a fold
// of their own in the page — which a diff makes of as few as one line.
func joinHunks(hunks []*udiff.Hunk, old string) []*udiff.Hunk {
	lines := slices.Collect(strings.Lines(old))
	joined := []*udiff.Hunk{hunks[0]}
	for _, hunk := range hunks[1:] {
		last := joined[len(joined)-1]
		end := last.FromLine // the line of old after last
		for _, l := range last.Lines {
			if l.Kind != udiff.Insert {
				end++
			}
		}
		if hunk.FromLine-end > udiff.DefaultContextLines || end < 1 || hunk.FromLine-1 > len(lines) || hunk.FromLine < end {
			joined = append(joined, hunk)
			continue
		}
		for _, line := range lines[end-1 : hunk.FromLine-1] {
			last.Lines = append(last.Lines, udiff.Line{Kind: udiff.Equal, Content: line})
		}
		last.Lines = append(last.Lines, hunk.Lines...)
	}
	return joined
}

// lineEdits are the edits that make after of before, of whole lines: what
// the two have in common stays whole lines, which a diff of their
// characters, matching what lines share in part, does not keep.
func lineEdits(before, after string) []udiff.Edit {
	ids := make(map[string]rune)
	// split numbers the lines of text, the same line the same, and says
	// where each starts, and where the last ends.
	split := func(text string) (lines []rune, starts []int) {
		starts = append(starts, 0)
		for line := range strings.Lines(text) {
			id, ok := ids[line]
			if !ok {
				id = rune(len(ids))
				ids[line] = id
			}
			lines = append(lines, id)
			starts = append(starts, starts[len(starts)-1]+len(line))
		}
		return lines, starts
	}
	a, from := split(before)
	b, to := split(after)
	diffs := lcs.DiffRunes(a, b)
	edits := make([]udiff.Edit, len(diffs))
	for i, d := range diffs {
		edits[i] = udiff.Edit{Start: from[d.Start], End: from[d.End], New: after[to[d.ReplStart]:to[d.ReplEnd]]}
	}
	return edits
}
