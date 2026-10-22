// Package surface reads the surfaces of the genui plugin
// (examples/plugins/genui): the versions its UI tool keeps in
// .harness/ui/<session>/<surface>/v<N>.json, the files they read, and their
// data model — JSON Pointers, the sources' formats, templates, numbers and a
// Metric's change — as the plugin's web script reads them (web/data.js), so
// the terminal and the iPhone app show a surface as the browser cockpit does.
//
// A surface's values are JSON's, as the page has them: nil, bool, float64,
// string, []any and *Object, an object that keeps its members' order — a
// table without columns shows its rows' fields in the order the file has
// them, as the page does.
package surface

import (
	"bytes"
	"errors"
	"io"
	"math"
	"strconv"
	"strings"
	"unicode/utf8"

	"encoding/json/jsontext"
)

// Object is a JSON object that keeps the order of its members.
type Object struct {
	keys   []string
	values map[string]any
}

// NewObject makes an empty object.
func NewObject() *Object { return &Object{values: map[string]any{}} }

// Get is the value of a member, and whether the object has it.
func (o *Object) Get(key string) (any, bool) {
	if o == nil {
		return nil, false
	}
	v, ok := o.values[key]
	return v, ok
}

// Has says whether the object has a member.
func (o *Object) Has(key string) bool {
	_, ok := o.Get(key)
	return ok
}

// Set sets a member: a new one goes last, one it has keeps its place.
func (o *Object) Set(key string, value any) {
	if _, ok := o.values[key]; !ok {
		o.keys = append(o.keys, key)
	}
	o.values[key] = value
}

// Delete removes a member.
func (o *Object) Delete(key string) {
	if _, ok := o.values[key]; !ok {
		return
	}
	delete(o.values, key)
	for i, k := range o.keys {
		if k == key {
			o.keys = append(o.keys[:i:i], o.keys[i+1:]...)
			break
		}
	}
}

// Keys are the members' names, in order.
func (o *Object) Keys() []string {
	if o == nil {
		return nil
	}
	return o.keys
}

// Len is how many members the object has.
func (o *Object) Len() int {
	if o == nil {
		return 0
	}
	return len(o.keys)
}

// MarshalJSON writes the object as JSON.stringify does, in its order.
func (o *Object) MarshalJSON() ([]byte, error) { return []byte(Encode(o)), nil }

// Clone copies a value deeply.
func Clone(v any) any {
	switch v := v.(type) {
	case *Object:
		if v == nil {
			return (*Object)(nil)
		}
		out := &Object{keys: append([]string(nil), v.keys...), values: make(map[string]any, len(v.values))}
		for k, x := range v.values {
			out.values[k] = Clone(x)
		}
		return out
	case []any:
		out := make([]any, len(v))
		for i, x := range v {
			out[i] = Clone(x)
		}
		return out
	}
	return v
}

// Decode reads a JSON text into a value, keeping objects' order. Numbers
// are float64s, as the page's; a name twice keeps the last value, as
// JSON.parse does.
func Decode(data []byte) (any, error) {
	dec := jsontext.NewDecoder(bytes.NewReader(data), jsontext.AllowDuplicateNames(true), jsontext.AllowInvalidUTF8(true))
	v, err := decodeValue(dec, 0)
	if err != nil {
		return nil, err
	}
	if _, err := dec.ReadToken(); err != io.EOF {
		if err == nil {
			err = errors.New("more after the value")
		}
		return nil, err
	}
	return v, nil
}

// DecodeObject reads a JSON object.
func DecodeObject(data []byte) (*Object, error) {
	v, err := Decode(data)
	if err != nil {
		return nil, err
	}
	o, ok := v.(*Object)
	if !ok {
		return nil, errors.New("not an object")
	}
	return o, nil
}

const maxDepth = 512

func decodeValue(dec *jsontext.Decoder, depth int) (any, error) {
	if depth > maxDepth {
		return nil, errors.New("nested too deep")
	}
	tok, err := dec.ReadToken()
	if err != nil {
		return nil, err
	}
	switch tok.Kind() {
	case 'n':
		return nil, nil
	case 'f':
		return false, nil
	case 't':
		return true, nil
	case '"':
		return tok.String(), nil
	case '0':
		f, err := tok.Float() // out of range: ±Inf, as JSON.parse
		if err != nil && !math.IsInf(f, 0) {
			return nil, err
		}
		return f, nil
	case '[':
		list := []any{}
		for {
			switch dec.PeekKind() {
			case ']':
				_, err := dec.ReadToken()
				return list, err
			case 0:
				_, err := dec.ReadToken()
				if err == nil {
					err = io.ErrUnexpectedEOF
				}
				return nil, err
			}
			v, err := decodeValue(dec, depth+1)
			if err != nil {
				return nil, err
			}
			list = append(list, v)
		}
	case '{':
		obj := NewObject()
		for {
			switch dec.PeekKind() {
			case '}':
				_, err := dec.ReadToken()
				return obj, err
			case 0:
				_, err := dec.ReadToken()
				if err == nil {
					err = io.ErrUnexpectedEOF
				}
				return nil, err
			}
			tok, err := dec.ReadToken()
			if err != nil {
				return nil, err
			}
			name := tok.String() // before the next read voids the token
			v, err := decodeValue(dec, depth+1)
			if err != nil {
				return nil, err
			}
			obj.Set(name, v)
		}
	}
	return nil, errors.New("unexpected " + tok.Kind().String())
}

// Encode writes a value as JSON.stringify does: compact, and with no more
// escaped than JSON needs.
func Encode(v any) string {
	var b strings.Builder
	encode(&b, v, 0)
	return b.String()
}

func encode(b *strings.Builder, v any, depth int) {
	if depth > maxDepth {
		b.WriteString("null")
		return
	}
	switch v := v.(type) {
	case nil:
		b.WriteString("null")
	case bool:
		if v {
			b.WriteString("true")
		} else {
			b.WriteString("false")
		}
	case float64:
		b.WriteString(jsNumber(v))
	case int:
		b.WriteString(strconv.Itoa(v))
	case int64:
		b.WriteString(strconv.FormatInt(v, 10))
	case string:
		quote(b, v)
	case []any:
		b.WriteByte('[')
		for i, x := range v {
			if i > 0 {
				b.WriteByte(',')
			}
			encode(b, x, depth+1)
		}
		b.WriteByte(']')
	case *Object:
		if v == nil {
			b.WriteString("null")
			return
		}
		b.WriteByte('{')
		for i, k := range v.keys {
			if i > 0 {
				b.WriteByte(',')
			}
			quote(b, k)
			b.WriteByte(':')
			encode(b, v.values[k], depth+1)
		}
		b.WriteByte('}')
	case map[string]any:
		// Of Go's own making: in the order of its names.
		o := NewObject()
		for _, k := range sortedKeys(v) {
			o.Set(k, v[k])
		}
		encode(b, o, depth)
	default:
		b.WriteString("null")
	}
}

// jsNumber is a number as JavaScript writes it.
func jsNumber(v float64) string {
	switch {
	case math.IsNaN(v), math.IsInf(v, 0):
		return "null"
	case v == 0:
		return "0"
	}
	a := math.Abs(v)
	if a >= 1e-6 && a < 1e21 {
		return strconv.FormatFloat(v, 'f', -1, 64)
	}
	s := strconv.FormatFloat(v, 'e', -1, 64) // 1e-07, 1.5e+21
	mantissa, exponent, _ := strings.Cut(s, "e")
	sign := exponent[:1]
	digits := strings.TrimLeft(exponent[1:], "0")
	if digits == "" {
		digits = "0"
	}
	return mantissa + "e" + sign + digits
}

func quote(b *strings.Builder, s string) {
	const hex = "0123456789abcdef"
	b.WriteByte('"')
	for i := 0; i < len(s); {
		c := s[i]
		if c < utf8.RuneSelf {
			switch {
			case c == '"':
				b.WriteString(`\"`)
			case c == '\\':
				b.WriteString(`\\`)
			case c == '\n':
				b.WriteString(`\n`)
			case c == '\r':
				b.WriteString(`\r`)
			case c == '\t':
				b.WriteString(`\t`)
			case c == '\b':
				b.WriteString(`\b`)
			case c == '\f':
				b.WriteString(`\f`)
			case c < 0x20:
				b.WriteString(`\u00`)
				b.WriteByte(hex[c>>4])
				b.WriteByte(hex[c&0xf])
			default:
				b.WriteByte(c)
			}
			i++
			continue
		}
		r, size := utf8.DecodeRuneInString(s[i:])
		if r == utf8.RuneError && size == 1 {
			b.WriteString(`\ufffd`)
		} else {
			b.WriteString(s[i : i+size])
		}
		i += size
	}
	b.WriteByte('"')
}

func sortedKeys(m map[string]any) []string {
	keys := make([]string, 0, len(m))
	for k := range m {
		keys = append(keys, k)
	}
	for i := 1; i < len(keys); i++ {
		for j := i; j > 0 && keys[j] < keys[j-1]; j-- {
			keys[j], keys[j-1] = keys[j-1], keys[j]
		}
	}
	return keys
}

// ObjectOf is a value as an object, or nil.
func ObjectOf(v any) *Object {
	o, _ := v.(*Object)
	return o
}

// ListOf is a value as a list, or nil.
func ListOf(v any) []any {
	l, _ := v.([]any)
	return l
}

// Str is a member of an object as a string, or "".
func Str(o *Object, key string) string {
	v, _ := o.Get(key)
	s, _ := v.(string)
	return s
}

// Int is a member of an object as a whole number, and whether it is one.
func Int(o *Object, key string) (int, bool) {
	v, _ := o.Get(key)
	f, ok := v.(float64)
	if !ok || f != math.Trunc(f) || math.Abs(f) > 1<<53 {
		return 0, false
	}
	return int(f), true
}

// Bool is a member of an object as a bool: true only when it is true.
func Bool(o *Object, key string) bool {
	v, _ := o.Get(key)
	b, _ := v.(bool)
	return b
}
