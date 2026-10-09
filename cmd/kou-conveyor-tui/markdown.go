package main

import (
	"regexp"
	"strings"
	"unicode"
	"unicode/utf8"

	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
)

// span is a run of text with one style. Wrapping styles every word on its
// own, so each rendered line is self-contained: the viewport can start
// drawing at any line without inheriting half of a style.
type span struct {
	text  string
	style lipgloss.Style
}

// wrapSpans word-wraps spans to width. first prefixes the first line and rest
// every following one; both are already styled and count toward width.
func wrapSpans(spans []span, width int, first, rest string) []string {
	var lines []string
	var line strings.Builder
	prefix, used, space := first, 0, false
	avail := func() int { return max(1, width-ansi.StringWidth(prefix)) }
	flush := func() {
		lines = append(lines, prefix+line.String())
		line.Reset()
		prefix, used, space = rest, 0, false
	}
	for _, s := range spans {
		for _, word := range splitWords(s.text) {
			if word == " " {
				space = used > 0
				continue
			}
			w := ansi.StringWidth(word)
			gap := 0
			if space {
				gap = 1
			}
			if used > 0 && used+gap+w > avail() {
				flush()
				gap = 0
			}
			// Only a word that alone is wider than the line gets here with
			// used == 0; it is broken hard.
			for w > avail() {
				head := ansi.Truncate(word, avail(), "")
				if head == "" {
					_, size := utf8.DecodeRuneInString(word)
					head = word[:size]
				}
				line.WriteString(s.style.Render(head))
				used = ansi.StringWidth(head)
				word = word[len(head):]
				w = ansi.StringWidth(word)
				flush()
			}
			if w == 0 {
				continue
			}
			if gap == 1 {
				line.WriteByte(' ')
				used++
			}
			line.WriteString(s.style.Render(word))
			used += w
			space = false
		}
	}
	if used > 0 || len(lines) == 0 {
		flush()
	}
	return lines
}

// splitWords splits text into words and single-space separators.
func splitWords(text string) []string {
	var out []string
	start := -1
	for i, r := range text {
		if unicode.IsSpace(r) {
			if start >= 0 {
				out = append(out, text[start:i])
				start = -1
			}
			if len(out) == 0 || out[len(out)-1] != " " {
				out = append(out, " ")
			}
			continue
		}
		if start < 0 {
			start = i
		}
	}
	if start >= 0 {
		out = append(out, text[start:])
	}
	return out
}

func wrapText(text string, style lipgloss.Style, width int, first, rest string) []string {
	var lines []string
	for _, line := range strings.Split(text, "\n") {
		lines = append(lines, wrapSpans([]span{{line, style}}, width, first, rest)...)
		first = rest
	}
	return lines
}

var (
	mdFence   = regexp.MustCompile("^\\s{0,3}(`{3,}|~{3,})\\s*([\\w+#.-]*)")
	mdHeading = regexp.MustCompile(`^\s{0,3}(#{1,6})\s+(.*?)\s*#*\s*$`)
	mdRule    = regexp.MustCompile(`^\s{0,3}(?:(?:-\s*){3,}|(?:\*\s*){3,}|(?:_\s*){3,})$`)
	mdQuote   = regexp.MustCompile(`^\s{0,3}>\s?(.*)$`)
	mdItem    = regexp.MustCompile(`^(\s*)([-*+]|\d{1,9}[.)])\s+(.*)$`)
	mdInline  = regexp.MustCompile("`([^`]+)`" + `|\*\*(\S(?:[^*]*\S)?)\*\*|__(\S(?:[^_]*\S)?)__|~~(\S(?:[^~]*\S)?)~~|\*(\S(?:[^*]*\S)?)\*|\[([^\]]+)\]\((https?://[^)\s]+)\)`)
)

// inlineSpans styles code, strong, emphasis, strike-through and links.
func inlineSpans(st styles, text string, base lipgloss.Style) []span {
	var spans []span
	last := 0
	for _, m := range mdInline.FindAllStringSubmatchIndex(text, -1) {
		if m[0] > last {
			spans = append(spans, span{text[last:m[0]], base})
		}
		group := func(n int) string {
			if m[2*n] < 0 {
				return ""
			}
			return text[m[2*n]:m[2*n+1]]
		}
		switch {
		case m[2] >= 0:
			spans = append(spans, span{group(1), st.code})
		case m[4] >= 0:
			spans = append(spans, span{group(2), base.Bold(true)})
		case m[6] >= 0:
			spans = append(spans, span{group(3), base.Bold(true)})
		case m[8] >= 0:
			spans = append(spans, span{group(4), base.Strikethrough(true)})
		case m[10] >= 0:
			spans = append(spans, span{group(5), base.Italic(true)})
		case m[12] >= 0:
			spans = append(spans, span{group(6), st.link}, span{" (" + group(7) + ")", st.faint})
		}
		last = m[1]
	}
	if last < len(text) {
		spans = append(spans, span{text[last:], base})
	}
	return spans
}

// renderMarkdown renders the Markdown agents actually write, in base style.
// Input is already free of terminal escapes (cockpit.Clean), so model output
// cannot smuggle styling or cursor movement past this renderer.
func renderMarkdown(st styles, text string, width int, base lipgloss.Style) []string {
	var out []string
	blank := func() {
		if len(out) > 0 && out[len(out)-1] != "" {
			out = append(out, "")
		}
	}
	lines := strings.Split(strings.ReplaceAll(text, "\t", "    "), "\n")
	for i := 0; i < len(lines); i++ {
		line := lines[i]
		switch {
		case strings.TrimSpace(line) == "":
			blank()
		case mdFence.MatchString(line):
			fence := mdFence.FindStringSubmatch(line)
			lang := fence[2]
			if lang == "" {
				lang = "code"
			}
			blank()
			out = append(out, st.rule.Render("┌ ")+st.faint.Render(strings.ToUpper(lang)))
			closer := string(fence[1][0])
			for i++; i < len(lines); i++ {
				trimmed := strings.TrimSpace(lines[i])
				if strings.HasPrefix(trimmed, strings.Repeat(closer, len(fence[1]))) && strings.Trim(trimmed, closer) == "" {
					break
				}
				for _, part := range strings.Split(ansi.Hardwrap(lines[i], max(1, width-2), true), "\n") {
					out = append(out, st.rule.Render("│ ")+st.codeBlock.Render(part))
				}
			}
			out = append(out, st.rule.Render("└"))
			blank()
		case mdHeading.MatchString(line):
			heading := mdHeading.FindStringSubmatch(line)
			style := st.bold
			if len(heading[1]) > 2 {
				style = st.label
			}
			blank()
			out = append(out, wrapSpans(inlineSpans(st, heading[2], style), width, "", "")...)
		case mdRule.MatchString(line):
			out = append(out, st.rule.Render(strings.Repeat("─", max(1, min(width, 48)))))
		case mdQuote.MatchString(line):
			bar := st.rule.Render("┃ ")
			for ; i < len(lines) && mdQuote.MatchString(lines[i]); i++ {
				quoted := mdQuote.FindStringSubmatch(lines[i])[1]
				out = append(out, wrapSpans(inlineSpans(st, quoted, st.quote), width, bar, bar)...)
			}
			i--
		case mdItem.MatchString(line):
			item := mdItem.FindStringSubmatch(line)
			indent := len(item[1]) / 2 * 2
			marker := item[2]
			if !strings.ContainsAny(marker, "0123456789") {
				marker = "•"
			}
			body := item[3]
			if strings.HasPrefix(body, "[ ] ") || strings.HasPrefix(body, "[x] ") || strings.HasPrefix(body, "[X] ") {
				marker = map[bool]string{true: "☑", false: "☐"}[body[1] != ' ']
				body = body[4:]
			}
			first := strings.Repeat(" ", indent) + st.faint.Render(marker) + " "
			rest := strings.Repeat(" ", indent+ansi.StringWidth(marker)+1)
			out = append(out, wrapSpans(inlineSpans(st, body, base), width, first, rest)...)
		default:
			out = append(out, wrapSpans(inlineSpans(st, strings.TrimSpace(line), base), width, "", "")...)
		}
	}
	for len(out) > 0 && out[len(out)-1] == "" {
		out = out[:len(out)-1]
	}
	return out
}
