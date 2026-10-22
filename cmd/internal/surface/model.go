package surface

import (
	"math"
	"math/big"
	"regexp"
	"strconv"
	"strings"
)

// Limits, as the plugin's.
const (
	MaxRows  = 10000    // rows of a jsonl or csv source
	MaxText  = 64 << 10 // characters of a text source
	MaxParse = 5 << 20  // bytes of a source read to show it
)

var (
	// Template is a reference in a text: {/pointer} a value, {#id} a Metric.
	Template = regexp.MustCompile(`\{(/[^{}]*|#[A-Za-z0-9][A-Za-z0-9_.-]*)\}`)
	number   = regexp.MustCompile(`^-?(\d+\.?\d*|\.\d+)([eE][-+]?\d+)?$`)
	integer  = regexp.MustCompile(`^-?\d+$`)
	natural  = regexp.MustCompile(`^\d+$`)
	lastRow  = regexp.MustCompile(`^/sources/([^/]+)/last/(.+)$`)
)

// ---------------------------------------------------------------- JSON Pointer

func tokens(pointer string) ([]string, bool) {
	if pointer == "" {
		return nil, true
	}
	if !strings.HasPrefix(pointer, "/") {
		return nil, false
	}
	list := strings.Split(pointer[1:], "/")
	for i, t := range list {
		list[i] = strings.ReplaceAll(strings.ReplaceAll(t, "~1", "/"), "~0", "~")
	}
	return list, true
}

// Get finds what pointer names in doc, and whether it is there. A negative
// index counts from the end of a list: -1 is its last element.
func Get(doc any, pointer string) (any, bool) {
	list, ok := tokens(pointer)
	if !ok {
		return nil, false
	}
	current := doc
	for _, token := range list {
		switch c := current.(type) {
		case []any:
			if !integer.MatchString(token) {
				return nil, false
			}
			i, err := strconv.Atoi(token)
			if err != nil {
				return nil, false
			}
			if i < 0 {
				i += len(c)
			}
			if i < 0 || i >= len(c) {
				return nil, false
			}
			current = c[i]
		case *Object:
			v, ok := c.Get(token)
			if !ok {
				return nil, false
			}
			current = v
		default:
			return nil, false
		}
	}
	return current, true
}

// Set sets (or with nil removes) what pointer names in doc, making the
// objects on the way.
func Set(doc *Object, pointer string, value any) {
	list, ok := tokens(pointer)
	if !ok || len(list) == 0 || doc == nil {
		return
	}
	setIn(doc, list, value)
}

func setIn(container any, list []string, value any) any {
	token := list[0]
	switch c := container.(type) {
	case *Object:
		if len(list) == 1 {
			if value == nil {
				c.Delete(token)
			} else {
				c.Set(token, value)
			}
			return c
		}
		next, _ := c.Get(token)
		switch next.(type) {
		case *Object, []any:
		default:
			next = NewObject()
		}
		c.Set(token, setIn(next, list[1:], value))
		return c
	case []any:
		if len(list) == 1 && token == "-" {
			if value != nil {
				return append(c, value)
			}
			return c
		}
		i := -1
		if natural.MatchString(token) {
			i, _ = strconv.Atoi(token)
		}
		if i < 0 || i >= len(c) {
			return c
		}
		if len(list) == 1 {
			if value == nil {
				return append(c[:i:i], c[i+1:]...)
			}
			c[i] = value
			return c
		}
		c[i] = setIn(c[i], list[1:], value)
		return c
	}
	return container
}

// Binding is the pointer a value binds to: an object of one member, "path".
func Binding(v any) (string, bool) {
	o, ok := v.(*Object)
	if !ok || o.Len() != 1 {
		return "", false
	}
	p, ok := o.values["path"].(string)
	return p, ok
}

// Resolve is a prop's value: what its binding finds, or the value itself.
func Resolve(doc, v any) (any, bool) {
	if p, ok := Binding(v); ok {
		return Get(doc, p)
	}
	return v, true
}

// ---------------------------------------------------------------- numbers and text

// AsNumber is a value as a number: a number, or a string that reads as one.
func AsNumber(v any) (float64, bool) {
	switch v := v.(type) {
	case float64:
		return v, true
	case string:
		t := strings.TrimSpace(v)
		if number.MatchString(t) {
			f, err := strconv.ParseFloat(t, 64)
			return f, err == nil || isRange(err)
		}
	}
	return 0, false
}

func isRange(err error) bool {
	e, ok := err.(*strconv.NumError)
	return ok && e.Err == strconv.ErrRange
}

// toFixed writes a non-negative number with digits after the point, as
// JavaScript's toFixed does: a tie goes up.
func toFixed(a float64, digits int) string {
	s := strconv.FormatFloat(a, 'f', digits, 64)
	shortest := strconv.FormatFloat(a, 'f', -1, 64)
	_, frac, _ := strings.Cut(shortest, ".")
	if len(frac) == digits+1 && frac[digits] == '5' {
		// The decimal that is the shortest may be the number exactly: a
		// tie, which Go rounds to even and JavaScript up.
		exact, ok := new(big.Rat).SetString(shortest)
		if ok && exact.Cmp(new(big.Rat).SetFloat64(a)) == 0 {
			s = strconv.FormatFloat(math.Nextafter(a, math.Inf(1)), 'f', digits, 64)
		}
	}
	return s
}

// Num is a number as a surface shows it: digits fixed (digits >= 0), or
// else as few as say it (8.5, 41.6, 0.12), and thousands apart (1 204).
func Num(v float64, digits int) string {
	switch {
	case math.IsNaN(v):
		return "nan"
	case math.IsInf(v, 1):
		return "inf"
	case math.IsInf(v, -1):
		return "-inf"
	}
	fixed := digits >= 0
	d := digits
	if !fixed {
		a := math.Abs(v)
		switch {
		case v == math.Trunc(v) || a >= 100:
			d = 0
		case a >= 10:
			d = 1
		default:
			d = 2
		}
	}
	text := toFixed(math.Abs(v), min(d, 20))
	whole, part, hasPart := strings.Cut(text, ".")
	whole = group(whole)
	if hasPart {
		text = whole + "." + part
	} else {
		text = whole
	}
	if !fixed && strings.Contains(text, ".") {
		text = strings.TrimRight(strings.TrimRight(text, "0"), ".")
	}
	if v < 0 {
		return "-" + text
	}
	return text
}

// group puts a space between thousands: 1204 → 1 204.
func group(whole string) string {
	if len(whole) <= 3 {
		return whole
	}
	var b strings.Builder
	first := len(whole) % 3
	if first > 0 {
		b.WriteString(whole[:first])
	}
	for i := first; i < len(whole); i += 3 {
		if b.Len() > 0 {
			b.WriteByte(' ')
		}
		b.WriteString(whole[i : i+3])
	}
	return b.String()
}

// Shown is a value as text: "—" for none, JSON for an object or a list.
func Shown(v any, ok bool) string {
	if !ok || v == nil {
		return "—"
	}
	switch v := v.(type) {
	case bool:
		if v {
			return "true"
		}
		return "false"
	case float64:
		return Num(v, -1)
	case string:
		return v
	case *Object, []any:
		return Encode(v)
	}
	return Encode(v)
}

// Change is a Metric's change from before: "▲8%", up (1), down (-1) or
// none (0).
type Change struct {
	Text string
	Dir  int
}

// Delta is the change from previous to value, if there is one to tell.
func Delta(value any, vok bool, previous any, pok bool) (Change, bool) {
	if !vok || !pok {
		return Change{}, false
	}
	v, ok1 := AsNumber(value)
	p, ok2 := AsNumber(previous)
	if !ok1 || !ok2 || p == 0 {
		return Change{}, false
	}
	pct := (v - p) / math.Abs(p) * 100
	if math.IsNaN(pct) {
		return Change{}, false
	}
	if math.Abs(pct) < 0.05 {
		return Change{Text: "=0%", Dir: 0}, true
	}
	size := toFixed(math.Abs(pct), 0)
	if math.Abs(pct) < 1 {
		size = toFixed(math.Abs(pct), 1)
	}
	if pct > 0 {
		return Change{Text: "▲" + size + "%", Dir: 1}, true
	}
	return Change{Text: "▼" + size + "%", Dir: -1}, true
}

// PreviousOf is a Metric's previous value: its own, or the row before the
// last of the source its value is the last row of.
func PreviousOf(doc any, c *Object) (any, bool) {
	if prev, ok := c.Get("previous"); ok {
		return Resolve(doc, prev)
	}
	value, _ := c.Get("value")
	if p, ok := Binding(value); ok {
		if m := lastRow.FindStringSubmatch(p); m != nil {
			return Get(doc, "/sources/"+m[1]+"/rows/-2/"+m[2])
		}
	}
	return nil, false
}

// MetricParts are a Metric's parts: its value as text, its unit, and its
// change, good or bad as the Metric's better says ("good", "bad", "flat").
type MetricParts struct {
	Text, Unit string
	Change     *Change
	Tone       string
	Missing    bool
}

// Metric is how a Metric shows.
func Metric(doc any, c *Object) MetricParts {
	raw, _ := c.Get("value")
	value, ok := Resolve(doc, raw)
	var parts MetricParts
	if f, numeric := value.(float64); numeric && ok {
		digits := -1
		if d, isInt := Int(c, "digits"); isInt && d >= 0 {
			digits = d
		}
		parts.Text = Num(f, digits)
	} else {
		parts.Text = Shown(value, ok)
	}
	parts.Missing = !ok || value == nil
	if unit, _ := c.Get("unit"); truthy(unit) && !parts.Missing {
		parts.Unit = Shown(unit, true)
		if s, isString := unit.(string); isString {
			parts.Unit = s
		}
	}
	previous, pok := PreviousOf(doc, c)
	parts.Tone = "flat"
	if change, has := Delta(value, ok, previous, pok); has {
		parts.Change = &change
		better := Str(c, "better")
		if change.Dir != 0 && (better == "lower" || better == "higher") {
			if (change.Dir < 0) == (better == "lower") {
				parts.Tone = "good"
			} else {
				parts.Tone = "bad"
			}
		}
	}
	return parts
}

func truthy(v any) bool {
	switch v := v.(type) {
	case nil:
		return false
	case bool:
		return v
	case float64:
		return v != 0 && !math.IsNaN(v)
	case string:
		return v != ""
	}
	return true
}

// MetricText is a Metric as a template's {#id} shows it: "41.6 ms ▲3%".
func MetricText(doc any, c *Object) string {
	m := Metric(doc, c)
	text := m.Text
	if m.Unit != "" {
		text += " " + m.Unit
	}
	if m.Change != nil {
		text += " " + m.Change.Text
	}
	return text
}

// Fill puts values in a template: {/pointer} a value of doc, {#id} a
// Metric of comps as it shows.
func Fill(doc any, comps map[string]*Object, template string) string {
	return Template.ReplaceAllStringFunc(template, func(whole string) string {
		ref := whole[1 : len(whole)-1]
		if strings.HasPrefix(ref, "#") {
			if c := comps[ref[1:]]; c != nil {
				return MetricText(doc, c)
			}
			return "?"
		}
		return Shown(Get(doc, ref))
	})
}

// FillItem fills a List's template with an item's fields.
func FillItem(item any, template string) string {
	return Template.ReplaceAllStringFunc(template, func(whole string) string {
		ref := whole[1 : len(whole)-1]
		if strings.HasPrefix(ref, "/") {
			return Shown(Get(item, ref))
		}
		return whole
	})
}

// TextOf is a text prop as it shows: a template filled, or a value bound.
func TextOf(doc any, comps map[string]*Object, value any) string {
	v, ok := Resolve(doc, value)
	if !ok {
		return "—"
	}
	if s, isString := v.(string); isString {
		return Fill(doc, comps, s)
	}
	return Shown(v, true)
}

// Label is a component's label as it shows, or "".
func Label(doc any, comps map[string]*Object, c *Object, key string) string {
	v, ok := c.Get(key)
	if !ok {
		return ""
	}
	return TextOf(doc, comps, v)
}
