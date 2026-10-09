// Package toolarg reads the arguments models give tools the way models
// write them, which is not always the way the tools' schemas say.
package toolarg

import (
	"encoding/json/v2"
	"fmt"
	"math"
	"strconv"
	"strings"
)

// Number is a number among a tool's arguments. Models write one as 100,
// 100.0 or "100" alike, though the schema says integer; each is read as
// 100. Anything else is an error that says a number is expected, with what
// was given.
type Number float64

// UnmarshalJSON reads a JSON number, or a JSON string that holds one. A
// JSON null leaves the number as it is.
func (n *Number) UnmarshalJSON(data []byte) error {
	text := strings.TrimSpace(string(data))
	if text == "null" {
		return nil
	}
	if strings.HasPrefix(text, `"`) {
		var quoted string
		if err := json.Unmarshal([]byte(text), &quoted); err != nil {
			return fmt.Errorf("want a number, such as 100, not %s", clip(text))
		}
		text = strings.TrimSpace(quoted)
	}
	value, err := strconv.ParseFloat(text, 64)
	if err != nil || math.IsNaN(value) || math.IsInf(value, 0) {
		return fmt.Errorf("want a number, such as 100, not %s", clip(string(data)))
	}
	*n = Number(value)
	return nil
}

// Whole is the number as a whole number, and false when it has a
// fraction, such as 1.5, or is too large to be exact.
func (n Number) Whole() (int, bool) {
	value := float64(n)
	if value != math.Trunc(value) || math.Abs(value) > 1<<53 {
		return 0, false
	}
	return int(value), true
}

// Rounded is the number rounded to the nearest whole number.
func (n Number) Rounded() int {
	return int(math.Round(float64(n)))
}

// Count reads a whole number that must not be negative, such as a line
// offset, from an argument called name; nil reads as 0, which tools take
// for their default. The error says what is expected.
func Count(name string, n *Number) (int, error) {
	if n == nil {
		return 0, nil
	}
	value, ok := n.Whole()
	if !ok {
		return 0, fmt.Errorf("argument %q must be a whole number, not %v", name, float64(*n))
	}
	if value < 0 {
		return 0, fmt.Errorf("argument %q must not be negative", name)
	}
	return value, nil
}

func clip(text string) string {
	if len(text) > 40 {
		return text[:40] + "…"
	}
	return text
}
