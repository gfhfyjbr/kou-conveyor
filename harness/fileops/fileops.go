// Package fileops reads and changes files the way the file tools do: Read
// shows a file's lines numbered, Edit replaces one string with another,
// Write replaces a file whole, and ApplyPatch applies a patch in the
// apply_patch format (patch.go). The functions are pure file system work,
// for the file operation and the code runtime to share; their errors are
// written for the model to read and act on.
package fileops

import (
	"bufio"
	"bytes"
	"errors"
	"fmt"
	"io"
	"os"
	"path"
	"path/filepath"
	"strings"
	"unicode/utf8"
)

const (
	// DefaultReadLimit is how many lines Read shows unless asked otherwise.
	DefaultReadLimit = 2000
	// MaxReadLimit bounds the lines one Read shows.
	MaxReadLimit = 20_000
	// DefaultMaxLineBytes is where a line is cut short.
	DefaultMaxLineBytes = 2000
	// DefaultMaxReadBytes bounds the text one Read returns, about 10,000
	// tokens: every turn after it sends the text again, so a larger file is
	// read in parts or searched.
	DefaultMaxReadBytes = 40_000
	// MaxEditBytes bounds the files Edit rewrites: it holds the whole file.
	MaxEditBytes = 32 << 20
)

// ReadOptions say what Read shows: lines from Offset (1-based; 0 is the
// start), at most Limit of them (0 is DefaultReadLimit), each cut at
// MaxLineBytes (0 is DefaultMaxLineBytes), all within MaxBytes (0 is
// DefaultMaxReadBytes).
type ReadOptions struct {
	Offset       int
	Limit        int
	MaxLineBytes int
	MaxBytes     int
}

// ReadResult is what Read shows: the lines numbered as cat -n numbers them,
// with a note on what was left out.
type ReadResult struct {
	// Text is what the model reads.
	Text string
	// Lines counts the file's lines; From and To are the lines shown, 0
	// when none.
	Lines    int
	From, To int
	Size     int64
	// Truncated reports lines after To.
	Truncated bool
}

// Read shows a file's lines.
func Read(name string, options ReadOptions) (ReadResult, error) {
	info, err := os.Stat(name)
	if err != nil {
		return ReadResult{}, describe(err, name)
	}
	if info.IsDir() {
		return ReadResult{}, fmt.Errorf("%s is a directory; list it with Bash (ls -la %s)", name, shellQuote(name))
	}
	if options.Offset < 0 {
		return ReadResult{}, errors.New("offset must not be negative")
	}
	if options.Limit < 0 {
		return ReadResult{}, errors.New("limit must not be negative")
	}
	limit := options.Limit
	if limit == 0 {
		limit = DefaultReadLimit
	}
	limit = min(limit, MaxReadLimit)
	maxLine := options.MaxLineBytes
	if maxLine <= 0 {
		maxLine = DefaultMaxLineBytes
	}
	maxBytes := options.MaxBytes
	if maxBytes <= 0 {
		maxBytes = DefaultMaxReadBytes
	}
	file, err := os.Open(name)
	if err != nil {
		return ReadResult{}, describe(err, name)
	}
	defer file.Close()

	head := make([]byte, 8192)
	n, _ := io.ReadFull(file, head)
	head = head[:n]
	if bytes.IndexByte(head, 0) >= 0 {
		hint := ""
		if IsImagePath(name) {
			hint = "; ViewImage shows images"
		}
		return ReadResult{}, fmt.Errorf("%s is a binary file (%s)%s", name, formatBytes(info.Size()), hint)
	}
	if _, err := file.Seek(0, io.SeekStart); err != nil {
		return ReadResult{}, describe(err, name)
	}

	result := ReadResult{Size: info.Size()}
	if info.Size() == 0 {
		result.Text = "(empty file)"
		return result, nil
	}
	from := max(options.Offset, 1)
	var text strings.Builder
	reader := bufio.NewReaderSize(file, 64*1024)
	number := 0
	shown := 0
	written := 0
	for {
		line, err := reader.ReadString('\n')
		if len(line) == 0 && err != nil {
			if err != io.EOF {
				return ReadResult{}, describe(err, name)
			}
			break
		}
		number++
		if number >= from && shown < limit && written < maxBytes {
			line = strings.TrimSuffix(line, "\n")
			line = strings.TrimSuffix(line, "\r")
			if len(line) > maxLine {
				cut := maxLine
				for cut > 0 && !utf8.RuneStart(line[cut]) {
					cut--
				}
				line = line[:cut] + fmt.Sprintf(" … [line cut: %d more bytes]", len(line)-cut)
			}
			formatted := fmt.Sprintf("%6d\t%s\n", number, line)
			text.WriteString(formatted)
			written += len(formatted)
			shown++
			if result.From == 0 {
				result.From = number
			}
			result.To = number
		}
		if err == io.EOF {
			break
		}
	}
	result.Lines = number
	switch {
	case number == 0:
		result.Text = "(empty file)"
		return result, nil
	case result.From == 0:
		return result, fmt.Errorf("offset %d is past the end of %s, which has %s", from, name, count(number, "line", "lines"))
	}
	result.Truncated = result.To < number
	body := strings.TrimSuffix(text.String(), "\n")
	switch {
	case result.Truncated && shown < limit:
		// The bytes ran out before the lines did: a file this large is
		// better searched than read through.
		body += fmt.Sprintf("\n[Lines %d-%d of %d shown, about %d bytes, the most one read shows; read on with offset=%d, or find what you need with rg -n.]", result.From, result.To, number, maxBytes, result.To+1)
	case result.Truncated:
		body += fmt.Sprintf("\n[Lines %d-%d of %d shown; read on with offset=%d.]", result.From, result.To, number, result.To+1)
	case result.From > 1:
		body += fmt.Sprintf("\n[Lines %d-%d of %d shown.]", result.From, result.To, number)
	}
	result.Text = body
	return result, nil
}

// EditOptions say how Edit replaces: every occurrence, or the one there is.
type EditOptions struct {
	ReplaceAll bool
}

// EditResult says what Edit changed: how many replacements it made, the
// first changed line and a numbered snippet around it.
type EditResult struct {
	Replacements int
	Line         int
	Snippet      string
	Lines        int
}

// Edit replaces old with new in the file: the one occurrence there is, or
// every one with ReplaceAll. It is an error for old to be missing, or to
// appear more than once without ReplaceAll.
func Edit(name, old, replacement string, options EditOptions) (EditResult, error) {
	if old == "" {
		return EditResult{}, errors.New("old_string must not be empty; Write creates or replaces a file whole")
	}
	if old == replacement {
		return EditResult{}, errors.New("old_string and new_string are the same; nothing to change")
	}
	defer lockFiles(name)()
	info, err := os.Stat(name)
	if err != nil {
		return EditResult{}, describe(err, name)
	}
	if info.IsDir() {
		return EditResult{}, fmt.Errorf("%s is a directory", name)
	}
	if info.Size() > MaxEditBytes {
		return EditResult{}, fmt.Errorf("%s is %s, too large to edit; use Bash", name, formatBytes(info.Size()))
	}
	data, err := os.ReadFile(name)
	if err != nil {
		return EditResult{}, describe(err, name)
	}
	if bytes.IndexByte(data[:min(len(data), 8192)], 0) >= 0 {
		return EditResult{}, fmt.Errorf("%s is a binary file", name)
	}
	content := string(data)
	// A file with Windows line endings takes the strings with them.
	if !strings.Contains(content, old) && strings.Contains(content, "\r\n") && !strings.Contains(old, "\r") {
		old = strings.ReplaceAll(old, "\n", "\r\n")
		replacement = strings.ReplaceAll(replacement, "\n", "\r\n")
	}
	occurrences := strings.Count(content, old)
	switch {
	case occurrences == 0:
		return EditResult{}, notFound(name, content, old)
	case occurrences > 1 && !options.ReplaceAll:
		return EditResult{}, fmt.Errorf("old_string appears %d times in %s; include more of the surrounding lines so it appears once, or set replace_all to change every occurrence", occurrences, name)
	}
	first := strings.Index(content, old)
	updated := content
	replacements := 1
	if options.ReplaceAll {
		updated = strings.ReplaceAll(content, old, replacement)
		replacements = occurrences
	} else {
		updated = content[:first] + replacement + content[first+len(old):]
	}
	if err := replaceFile(name, []byte(updated), info.Mode().Perm()); err != nil {
		return EditResult{}, err
	}
	line := 1 + strings.Count(content[:first], "\n")
	changed := 1 + strings.Count(replacement, "\n")
	return EditResult{
		Replacements: replacements,
		Line:         line,
		Snippet:      snippet(updated, line, changed),
		Lines:        lineCount(updated),
	}, nil
}

// notFound explains a string the file does not hold, and where a near
// miss is: whitespace, or a shorter string that would match.
func notFound(name, content, old string) error {
	if strings.Contains(squeeze(content), squeeze(old)) {
		return fmt.Errorf("old_string was not found in %s as written, but matches when whitespace is ignored: check indentation (tabs against spaces), trailing spaces and blank lines, and copy the lines exactly as Read shows them", name)
	}
	lines := strings.Split(strings.TrimSpace(old), "\n")
	if len(lines) > 1 {
		if strings.Contains(content, strings.TrimSpace(lines[0])) {
			return fmt.Errorf("old_string was not found in %s; its first line is there, so a later line differs: copy the lines exactly as Read shows them", name)
		}
	}
	return fmt.Errorf("old_string was not found in %s; copy the lines exactly as Read shows them, without the line numbers", name)
}

// squeeze removes all whitespace, for a comparison that ignores it.
func squeeze(text string) string {
	return strings.Join(strings.Fields(text), "")
}

// snippet is a numbered view of the lines around a change of changed lines
// at line, with a few lines on each side.
func snippet(content string, line, changed int) string {
	const around = 4
	lines := strings.Split(content, "\n")
	if len(lines) > 0 && lines[len(lines)-1] == "" {
		lines = lines[:len(lines)-1]
	}
	from := max(1, line-around)
	to := min(len(lines), line+changed-1+around)
	var text strings.Builder
	for number := from; number <= to; number++ {
		fmt.Fprintf(&text, "%6d\t%s\n", number, lines[number-1])
	}
	return strings.TrimSuffix(text.String(), "\n")
}

// WriteResult says what Write did: the size written, and what it replaced.
type WriteResult struct {
	Bytes         int
	Lines         int
	Created       bool
	PreviousLines int
}

// Write replaces the file with content, creating it and its directories.
func Write(name, content string) (WriteResult, error) {
	result := WriteResult{Bytes: len(content), Lines: lineCount(content), Created: true}
	mode := os.FileMode(0o644)
	defer lockFiles(name)()
	info, err := os.Stat(name)
	switch {
	case err == nil && info.IsDir():
		return WriteResult{}, fmt.Errorf("%s is a directory", name)
	case err == nil:
		result.Created = false
		mode = info.Mode().Perm()
		if info.Size() <= MaxEditBytes {
			if previous, readErr := os.ReadFile(name); readErr == nil {
				result.PreviousLines = lineCount(string(previous))
			}
		}
	case !errors.Is(err, os.ErrNotExist):
		return WriteResult{}, describe(err, name)
	}
	if err := os.MkdirAll(filepath.Dir(name), 0o755); err != nil {
		return WriteResult{}, fmt.Errorf("create the directory of %s: %w", name, err)
	}
	if err := replaceFile(name, []byte(content), mode); err != nil {
		return WriteResult{}, err
	}
	return result, nil
}

// replaceFile writes data to name whole: a temporary file beside it takes
// its place, so a reader never sees half of it. A symbolic link stays one:
// the file it leads to is the one replaced.
func replaceFile(name string, data []byte, mode os.FileMode) error {
	if target, err := filepath.EvalSymlinks(name); err == nil {
		name = target
	}
	directory := filepath.Dir(name)
	temporary, err := os.CreateTemp(directory, "."+filepath.Base(name)+".*.tmp")
	if err != nil {
		return fmt.Errorf("write %s: %w", name, err)
	}
	temporaryName := temporary.Name()
	_, err = temporary.Write(data)
	if err == nil {
		err = temporary.Chmod(mode)
	}
	if closeErr := temporary.Close(); err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Rename(temporaryName, name)
	}
	if err != nil {
		os.Remove(temporaryName)
		return fmt.Errorf("write %s: %w", name, err)
	}
	return nil
}

func lineCount(content string) int {
	if content == "" {
		return 0
	}
	n := strings.Count(content, "\n")
	if !strings.HasSuffix(content, "\n") {
		n++
	}
	return n
}

// describe makes a file error readable: the path and what went wrong.
func describe(err error, name string) error {
	switch {
	case errors.Is(err, os.ErrNotExist):
		return fmt.Errorf("%s does not exist", name)
	case errors.Is(err, os.ErrPermission):
		return fmt.Errorf("%s: permission denied", name)
	}
	return fmt.Errorf("%s: %v", name, errors.Unwrap(err))
}

// IsImagePath reports a file name of an image.
func IsImagePath(name string) bool {
	switch strings.ToLower(path.Ext(filepath.ToSlash(name))) {
	case ".png", ".jpg", ".jpeg", ".gif", ".webp", ".bmp", ".tif", ".tiff":
		return true
	}
	return false
}

func count(n int, one, many string) string {
	if n == 1 {
		return "1 " + one
	}
	return fmt.Sprintf("%d %s", n, many)
}

func formatBytes(n int64) string {
	switch {
	case n < 1<<10:
		return count(int(n), "byte", "bytes")
	case n < 1<<20:
		return fmt.Sprintf("%.1f KB", float64(n)/(1<<10))
	case n < 1<<30:
		return fmt.Sprintf("%.1f MB", float64(n)/(1<<20))
	}
	return fmt.Sprintf("%.1f GB", float64(n)/(1<<30))
}

func shellQuote(value string) string {
	safe := true
	for _, r := range value {
		if !(r >= 'a' && r <= 'z' || r >= 'A' && r <= 'Z' || r >= '0' && r <= '9' || strings.ContainsRune("_./@%+=:,-", r)) {
			safe = false
			break
		}
	}
	if safe && value != "" && !strings.HasPrefix(value, "-") {
		return value
	}
	return "'" + strings.ReplaceAll(value, "'", `'\''`) + "'"
}
