package surface

import (
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"
	"unicode/utf8"
)

// Formats are the formats a source can be read as.
var Formats = []string{"json", "jsonl", "csv", "tsv", "text", "markdown"}

// InferFormat is the format a file's extension says.
func InferFormat(file string) string {
	switch strings.ToLower(path.Ext(file)) {
	case ".json":
		return "json"
	case ".jsonl", ".ndjson":
		return "jsonl"
	case ".csv":
		return "csv"
	case ".tsv":
		return "tsv"
	case ".md", ".markdown":
		return "markdown"
	}
	return "text"
}

func cell(text string) any {
	t := strings.TrimSpace(text)
	if number.MatchString(t) {
		if f, err := strconv.ParseFloat(t, 64); err == nil || isRange(err) {
			return f
		}
	}
	return text
}

// csvRows reads CSV as Python's csv.reader does by default: quotes start a
// field, "" in one is a quote, and a blank line is an empty row.
func csvRows(text string, delimiter rune) [][]string {
	var rows [][]string
	var row []string
	var field strings.Builder
	quoted, wasQuoted := false, false
	runes := []rune(text)
	for i := 0; i < len(runes); i++ {
		c := runes[i]
		if quoted {
			if c == '"' {
				if i+1 < len(runes) && runes[i+1] == '"' {
					field.WriteRune('"')
					i++
				} else {
					quoted = false
				}
			} else {
				field.WriteRune(c)
			}
			continue
		}
		switch {
		case c == '"' && field.Len() == 0 && !wasQuoted:
			quoted, wasQuoted = true, true
		case c == delimiter:
			row = append(row, field.String())
			field.Reset()
			wasQuoted = false
		case c == '\n' || c == '\r':
			if c == '\r' && i+1 < len(runes) && runes[i+1] == '\n' {
				i++
			}
			if len(row) != 0 || field.Len() != 0 || wasQuoted {
				row = append(row, field.String())
			}
			rows = append(rows, row)
			row = nil
			field.Reset()
			wasQuoted = false
		default:
			field.WriteRune(c)
		}
	}
	if len(row) != 0 || field.Len() != 0 || wasQuoted {
		row = append(row, field.String())
		rows = append(rows, row)
	}
	return rows
}

// lineBreak ends lines where Python's splitlines ends them.
var lineBreak = regexp.MustCompile("\r\n|[\n\r\v\f\x1c\x1d\x1e\u0085\u2028\u2029]")

func rowsValue(rows []any, columns []any) *Object {
	count := len(rows)
	if len(rows) > MaxRows {
		rows = rows[len(rows)-MaxRows:]
	}
	o := NewObject()
	if columns != nil {
		o.Set("columns", columns)
	}
	o.Set("rows", rows)
	if len(rows) != 0 {
		o.Set("last", rows[len(rows)-1])
	} else {
		o.Set("last", nil)
	}
	o.Set("count", float64(count))
	return o
}

// ParseSource is what a source's text is in the data model: jsonl, csv and
// tsv give {rows, last, count} (and csv's columns), json the value, text
// and markdown the text.
func ParseSource(format, text string) (any, error) {
	switch format {
	case "json":
		return Decode([]byte(text))
	case "jsonl":
		rows := []any{}
		for _, raw := range lineBreak.Split(text, -1) {
			line := strings.TrimSpace(raw)
			if line == "" {
				continue
			}
			if v, err := Decode([]byte(line)); err == nil {
				rows = append(rows, v) // a line being written, or a stray one, is skipped
			}
		}
		return rowsValue(rows, nil), nil
	case "csv", "tsv":
		delimiter := ','
		if format == "tsv" {
			delimiter = '\t'
		}
		table := csvRows(text, delimiter)
		if len(table) == 0 {
			return rowsValue([]any{}, []any{}), nil
		}
		names := make([]string, len(table[0]))
		columns := make([]any, len(table[0]))
		for i, c := range table[0] {
			names[i] = strings.TrimSpace(c)
			columns[i] = names[i]
		}
		rows := []any{}
		for _, r := range table[1:] {
			blank := true
			for _, c := range r {
				if strings.TrimSpace(c) != "" {
					blank = false
					break
				}
			}
			if blank {
				continue
			}
			row := NewObject()
			for i, c := range r {
				name := fmt.Sprintf("col%d", i+1)
				if i < len(names) && names[i] != "" {
					name = names[i]
				}
				row.Set(name, cell(c))
			}
			rows = append(rows, row)
		}
		return rowsValue(rows, columns), nil
	}
	return cutRunes(text, MaxText), nil
}

func cutRunes(text string, n int) string {
	if len(text) <= n {
		return text
	}
	count := 0
	for i := range text {
		if count == n {
			return text[:i]
		}
		count++
	}
	return text
}

// Hidden says whether a path of the workspace is one a surface does not
// show: under .git, or a .env file.
func Hidden(relative string) bool {
	parts := strings.Split(relative, "/")
	if parts[0] == ".git" {
		return true
	}
	for _, p := range parts {
		if p == ".env" || strings.HasPrefix(p, ".env.") {
			return true
		}
	}
	return false
}

// Clean is a workspace path from a surface, from the root and with
// slashes, or "" when it leads outside or is hidden.
func Clean(file string) string {
	file = strings.TrimSpace(strings.TrimPrefix(strings.TrimSpace(file), "$"))
	if file == "" || strings.HasPrefix(file, "/") || strings.Contains(file, "\\") {
		return ""
	}
	clean := path.Clean(file)
	if clean == "." || clean == ".." || strings.HasPrefix(clean, "../") || Hidden(clean) {
		return ""
	}
	return clean
}

func iso(t time.Time) string { return t.UTC().Format("2006-01-02T15:04:05Z") }

// Read reads what a surface shows of a file of the workspace: at most
// MaxParse bytes, or of a longer file of rows (cut), its last whole lines.
func readFile(root *os.Root, relative string, rows bool) (data []byte, info fs.FileInfo, cut bool, err error) {
	f, err := root.Open(relative)
	if err != nil {
		return nil, nil, false, err
	}
	defer f.Close()
	info, err = f.Stat()
	if err != nil {
		return nil, nil, false, err
	}
	if info.IsDir() {
		return nil, info, false, errFolder
	}
	data, err = io.ReadAll(io.LimitReader(f, MaxParse+1))
	if err != nil {
		return nil, info, false, err
	}
	if len(data) <= MaxParse {
		return data, info, false, nil
	}
	if !rows {
		return nil, info, false, errTooBig
	}
	// The newest rows are at the end: keep those, from a whole line.
	if _, err := f.Seek(max(0, info.Size()-MaxParse), io.SeekStart); err != nil {
		return nil, info, false, err
	}
	data, err = io.ReadAll(io.LimitReader(f, MaxParse))
	if err != nil {
		return nil, info, false, err
	}
	if i := strings.IndexByte(string(data), '\n'); i >= 0 {
		data = data[i+1:]
	}
	return data, info, true, nil
}

var (
	errFolder = errors.New("a folder, not a file")
	errTooBig = fmt.Errorf("bigger than %d MB", MaxParse>>20)
)

// Source is a file a surface reads, as its version declares it.
type Source struct {
	Name, File, Format string
}

// SourcesOf are the sources a version declares, in its order.
func SourcesOf(version *Object) []Source {
	decls := ObjectOf(must(version.Get("sources")))
	var out []Source
	for _, name := range decls.Keys() {
		d := ObjectOf(must(decls.Get(name)))
		file := Str(d, "file")
		if file == "" {
			continue
		}
		format := Str(d, "format")
		if !validFormat(format) {
			format = InferFormat(file)
		}
		out = append(out, Source{Name: name, File: file, Format: format})
	}
	return out
}

func must(v any, _ bool) any { return v }

func validFormat(f string) bool {
	for _, x := range Formats {
		if x == f {
			return true
		}
	}
	return false
}

// ReadSource reads a source now: its value, whether it has one, and its
// sourceInfo — path, format, size, mtime, and why it has no value (error),
// or that it is the end of a longer file (cut).
func ReadSource(root *os.Root, s Source) (any, bool, *Object) {
	info := NewObject()
	info.Set("path", s.File)
	info.Set("format", s.Format)
	relative := Clean(s.File)
	if relative == "" {
		info.Set("error", "the file leads outside the workspace")
		return nil, false, info
	}
	rows := s.Format == "jsonl" || s.Format == "csv" || s.Format == "tsv"
	data, st, cut, err := readFile(root, relative, rows)
	if st != nil {
		info.Set("size", float64(st.Size()))
		info.Set("mtime", iso(st.ModTime()))
	}
	switch {
	case err == nil:
	case errors.Is(err, errFolder), errors.Is(err, errTooBig):
		info.Set("error", err.Error())
		return nil, false, info
	case errors.Is(err, fs.ErrNotExist):
		info.Set("error", "no such file")
		return nil, false, info
	case strings.Contains(err.Error(), "escapes"):
		info.Set("error", "the file leads outside the workspace")
		return nil, false, info
	default:
		info.Set("error", "no such file")
		return nil, false, info
	}
	if cut {
		info.Set("cut", true)
	}
	text := string(data)
	if !utf8.ValidString(text) {
		text = strings.ToValidUTF8(text, "\uFFFD")
	}
	value, err := ParseSource(s.Format, text)
	if err != nil {
		info.Set("error", fmt.Sprintf("not %s: %v", s.Format, err))
		return nil, false, info
	}
	return value, true, info
}

// Reader reads sources, and reads a file again only once it changed: a
// live surface asks for its files every second or two.
type Reader struct {
	mu    sync.Mutex
	cache map[string]*read
	order []string
}

type read struct {
	stamp string
	value any
	ok    bool
	info  *Object
}

const readerKeep = 64

// NewReader makes a reader.
func NewReader() *Reader { return &Reader{cache: map[string]*read{}} }

// Stamp names the state of a file: its size and time, or why it has none.
func Stamp(root *os.Root, file string) string {
	relative := Clean(file)
	if relative == "" {
		return "hidden"
	}
	st, err := root.Stat(relative)
	if err != nil {
		return "missing"
	}
	return fmt.Sprintf("%x-%x-%t", st.Size(), st.ModTime().UnixNano(), st.IsDir())
}

// Read reads a source, or gives what it read of it before when the file
// did not change since. Its values are shared: do not change them.
func (r *Reader) Read(root *os.Root, key string, s Source) (any, bool, *Object) {
	stamp := Stamp(root, s.File)
	id := key + "\x00" + s.File + "\x00" + s.Format
	r.mu.Lock()
	if c, ok := r.cache[id]; ok && c.stamp == stamp {
		r.mu.Unlock()
		return c.value, c.ok, c.info
	}
	r.mu.Unlock()
	value, ok, info := ReadSource(root, s)
	r.mu.Lock()
	defer r.mu.Unlock()
	if _, ok := r.cache[id]; !ok {
		r.order = append(r.order, id)
		if len(r.order) > readerKeep {
			delete(r.cache, r.order[0])
			r.order = r.order[1:]
		}
	}
	r.cache[id] = &read{stamp: stamp, value: value, ok: ok, info: info}
	return value, ok, info
}
