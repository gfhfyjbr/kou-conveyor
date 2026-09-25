package operation

import (
	"fmt"
	"unicode/utf8"
)

const (
	DefaultMaxOutputLength = 40_000
	MaxOutputLength        = 1_000_000
)

func BoundOutput(text string, limit int) (string, bool) {
	return boundOutput(text, "", int64(len(text)), limit, "")
}

func boundOutput(head, tail string, fullSize int64, limit int, path string) (string, bool) {
	limit = max(0, limit)
	if tail == "" {
		if fullSize != int64(len(head)) {
			panic("bound output: empty tail requires the complete output in head")
		}
		if utf8.RuneCountInString(head) <= limit {
			return sanitizeUTF8(head), false
		}
		tail = head
	}

	head, headSize := outputPart(head, limit/2, false)
	tail, tailSize := outputPart(tail, limit-limit/2, true)
	skipped := fullSize - int64(headSize+tailSize)
	marker := fmt.Sprintf("...%d bytes truncated", skipped)
	if path != "" {
		marker += "; complete output in " + path
	}
	return head + marker + "..." + tail, true
}

func outputPart(text string, limit int, tail bool) (string, int) {
	if tail {
		start := len(text)
		for start > 0 && limit > 0 {
			_, size := utf8.DecodeLastRuneInString(text[:start])
			start -= size
			limit--
		}
		return sanitizeUTF8(text[start:]), len(text) - start
	}
	end := 0
	for end < len(text) && limit > 0 {
		_, size := utf8.DecodeRuneInString(text[end:])
		end += size
		limit--
	}
	return sanitizeUTF8(text[:end]), end
}

func sanitizeUTF8(text string) string {
	return string([]rune(text))
}
