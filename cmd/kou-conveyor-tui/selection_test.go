package main

import (
	"strings"
	"testing"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
)

// fakeClipboard records what is copied instead of touching the system's.
func fakeClipboard(t *testing.T) *[]string {
	t.Helper()
	var copied []string
	saved := writeClipboard
	writeClipboard = func(text string) error {
		copied = append(copied, text)
		return nil
	}
	t.Cleanup(func() { writeClipboard = saved })
	return &copied
}

// dragAndCopy drags the left button and delivers what letting go copies.
func dragAndCopy(t *testing.T, m *uiModel, from, to cell, via ...cell) {
	t.Helper()
	m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionPress, X: from.col, Y: from.row})
	for _, c := range append(via, to) {
		m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionMotion, X: c.col, Y: c.row})
	}
	// While the button is down the selection shows, and the frame still fits.
	for i, line := range strings.Split(m.View(), "\n") {
		if w := ansi.StringWidth(line); w > m.width {
			t.Fatalf("line %d is %d wide", i, w)
		}
	}
	_, cmd := m.Update(tea.MouseMsg{Button: tea.MouseButtonLeft, Action: tea.MouseActionRelease, X: to.col, Y: to.row})
	if cmd != nil {
		m.Update(cmd())
	}
}

// find returns the screen cell where text shows in the transcript.
func find(t *testing.T, m *uiModel, text string) cell {
	t.Helper()
	for i, line := range m.lines {
		if col := strings.Index(ansi.Strip(line), text); col >= 0 {
			m.view.SetYOffset(max(0, i-2))
			return cell{row: m.top() + i - m.view.YOffset, col: ansi.StringWidth(ansi.Strip(line)[:col])}
		}
	}
	t.Fatalf("%q is not in the transcript", text)
	return cell{}
}

func TestSelectingTranscriptTextCopiesIt(t *testing.T) {
	copied := fakeClipboard(t)
	m := testModel(t)
	runPrompt(t, m, "first question")

	at := find(t, m, "echo: first question")
	dragAndCopy(t, m, at, cell{at.row, at.col + len("echo: first question") - 1})
	if len(*copied) != 1 || (*copied)[0] != "echo: first question" || m.toast != "20 chars copied" {
		t.Fatalf("copied %q, toast %q", *copied, m.toast)
	}
	if footer := ansi.Strip(m.footer()); !strings.Contains(footer, "20 chars copied") {
		t.Fatalf("footer = %q", footer)
	}
	m.Update(toastMsg{gen: m.toastGen})
	if m.toast != "" {
		t.Fatal("the toast stayed")
	}

	// From the prompt to the answer: rows after the first leave the gutter
	// of rails and times out.
	prompt := find(t, m, "first question")
	answer := find(t, m, "echo: first question")
	dragAndCopy(t, m, prompt, cell{answer.row, answer.col + 3})
	if got := (*copied)[1]; !strings.HasPrefix(got, "first question\n") || !strings.HasSuffix(got, "\necho") || strings.Contains(got, "│") {
		t.Fatalf("copied %q", got)
	}

	// A click copies nothing.
	click(m, answer.col, answer.row)
	if len(*copied) != 2 {
		t.Fatalf("a click copied %q", (*copied)[2:])
	}
}

func TestSelectionInTheComposerStaysThere(t *testing.T) {
	copied := fakeClipboard(t)
	m := testModel(t)
	runPrompt(t, m, "first question")
	words := strings.Repeat("lorem ipsum dolor sit amet ", 8) // wraps at this width
	m.input.SetValue(words + "\nsecond line")
	m.resize()
	rows, first, ok := m.composerRows()
	if !ok || first != 0 || len(rows) < 4 {
		t.Fatalf("rows %d, first %d, ok %v", len(rows), first, ok)
	}
	top := m.inputTop()

	// Dragged up into the transcript, the selection stops at the composer's
	// first row: it copies the text from its start, soft wraps joined.
	dragAndCopy(t, m, cell{top + 1, promptWidth + 5}, cell{m.top(), 10}, cell{top, 40})
	want := string([]rune(words)[:len(rows[0].runes)+6])
	if len(*copied) != 1 || (*copied)[0] != want {
		t.Fatalf("copied %q, want %q", *copied, want)
	}

	// Dragged down past it, the selection runs to its end, across the line
	// break.
	dragAndCopy(t, m, cell{top, promptWidth + 6}, cell{m.height - 1, 3})
	if got := (*copied)[1]; got != strings.TrimPrefix(m.input.Value(), "lorem ") {
		t.Fatalf("copied %q", got)
	}
}

// The composer's rows must be found for any text: wrapLine has to wrap as
// the textarea does.
func TestComposerRowsMatchTheTextarea(t *testing.T) {
	m := testModel(t)
	for _, width := range []int{100, 40, 23} {
		m.Update(tea.WindowSizeMsg{Width: width, Height: 40})
		for _, text := range []string{
			"short",
			strings.Repeat("word ", 40),
			strings.Repeat("x", 250),
			"two  spaces   and    more     " + strings.Repeat("y", 60),
			"日本語のテキストは幅が二倍です。" + strings.Repeat("漢字", 30),
			"emoji 🙂🙂 " + strings.Repeat("🚀", 40) + " end",
			"trailing spaces      \n\n\nafter empty lines\n",
			strings.Repeat("line\n", 30) + "last",
		} {
			m.input.SetValue(text)
			m.resize()
			for _, moved := range []bool{false, true} {
				if moved {
					// Typing makes the textarea scroll to the cursor.
					m.Update(key("up"))
					m.Update(key("down"))
				}
				if _, _, ok := m.composerRows(); !ok {
					t.Fatalf("width %d, %q: rows do not match the screen:\n%s", width, text, ansi.Strip(m.input.View()))
				}
			}
		}
	}
}
