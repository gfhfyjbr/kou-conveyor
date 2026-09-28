package agentrunner

import (
	"bufio"
	"bytes"
	"cmp"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"unicode/utf8"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
)

// Linked files. A message can link files and folders (its files): the
// runner reads what the model sees of each as it takes the message, and the
// session records that with the message (contextbuilder.LinkedFile), so the
// conversation reads the same when it is resumed. A file shows the lines
// its link asks for, or else its beginning, or the whole of a small file; a
// folder shows its entries. The model reads the rest from the file itself,
// which the text it gets says how to.

// RequestFile is a file or a folder a message links to. Path is relative
// to the workspace, absolute, or in the home folder (~/); StartLine and
// EndLine, counting from 1, ask for lines of a file: both, or where to
// start, or where to end. Label is how the message's text refers to it,
// "$" and the path by default.
type RequestFile struct {
	Label     string `json:"label"`
	Path      string `json:"path"`
	StartLine int    `json:"start_line"`
	EndLine   int    `json:"end_line"`
}

const (
	// maxLinkedFiles bounds the files and folders of one message.
	maxLinkedFiles = 20
	// A file of up to wholeLines lines and wholeBytes shows whole; a larger
	// one shows its first previewLines lines, within previewBytes, and so
	// do the lines from a StartLine on.
	wholeLines   = 600
	wholeBytes   = 20 << 10
	previewLines = 100
	previewBytes = 8 << 10
	// Lines asked for from StartLine to EndLine show up to rangeLines of
	// them, within rangeBytes.
	rangeLines = 1000
	rangeBytes = 64 << 10
	// lineBytes bounds what a line shows: minified code and logs have
	// lines of megabytes.
	lineBytes = 2000
	// messageBytes bounds what the files of one message show together:
	// the files after it show none of their lines.
	messageBytes = 160 << 10
	// Lines are counted in files of up to countBytes; lines are looked for
	// in the first scanBytes.
	countBytes = 32 << 20
	scanBytes  = 256 << 20
	// sniffBytes are looked at to tell a binary file, which has a NUL byte
	// in them, from text.
	sniffBytes = 8 << 10
	// folderEntries bounds the entries a folder shows.
	folderEntries = 200
)

// validateFiles checks the files a message links.
func validateFiles(files []RequestFile) error {
	if len(files) > maxLinkedFiles {
		return fmt.Errorf("a message links at most %d files, not %d", maxLinkedFiles, len(files))
	}
	for index, file := range files {
		switch {
		case strings.TrimSpace(file.Path) == "":
			return fmt.Errorf("file %d has no path", index+1)
		case strings.ContainsAny(file.Path, "\x00\n\r"):
			return fmt.Errorf("file %d: path must be one line without NUL bytes", index+1)
		case file.StartLine < 0 || file.EndLine < 0:
			return fmt.Errorf("file %d: lines count from 1", index+1)
		case file.EndLine != 0 && file.EndLine < file.StartLine:
			return fmt.Errorf("file %d: end_line %d is before start_line %d", index+1, file.EndLine, file.StartLine)
		}
	}
	return nil
}

// linkFiles reads what the model sees of the files a message links.
func linkFiles(workspace string, files []RequestFile) []contextbuilder.LinkedFile {
	if len(files) == 0 {
		return nil
	}
	budget := messageBytes
	linked := make([]contextbuilder.LinkedFile, 0, len(files))
	for _, file := range files {
		one := linkFile(workspace, file, budget)
		budget -= len(one.Content)
		linked = append(linked, one)
	}
	return linked
}

// linkFile reads what the model sees of one file or folder, showing at most
// budget bytes of it.
func linkFile(workspace string, request RequestFile, budget int) contextbuilder.LinkedFile {
	path, shown := resolveLink(workspace, strings.TrimSpace(request.Path))
	file := contextbuilder.LinkedFile{Label: strings.TrimSpace(request.Label), Path: shown}
	if file.Label == "" {
		file.Label = "$" + strings.TrimSpace(request.Path)
	}
	// Stat follows links; a FIFO or a device would never finish reading.
	info, err := os.Stat(path)
	switch {
	case err != nil:
		file.Error = describeError(err)
	case info.IsDir():
		file.Directory = true
		if !strings.HasSuffix(file.Path, "/") {
			file.Path += "/"
		}
		listFolder(path, &file, budget)
	case !info.Mode().IsRegular():
		file.Error = "not a regular file"
	default:
		file.Size = info.Size()
		readLines(path, &file, request.StartLine, request.EndLine, budget)
	}
	return file
}

// resolveLink returns where a linked path is, and the path the model is
// told: relative to the workspace, which is where its commands run, or
// absolute, with the home folder spelled out.
func resolveLink(workspace, path string) (string, string) {
	if rest, ok := strings.CutPrefix(path, "~/"); ok || path == "~" {
		if home, err := os.UserHomeDir(); err == nil {
			path = filepath.Join(home, rest)
			return path, path
		}
	}
	clean := filepath.Clean(path)
	if filepath.IsAbs(clean) {
		return clean, clean
	}
	return filepath.Join(workspace, clean), filepath.ToSlash(clean)
}

// describeError says why a file could not be read, without its path.
func describeError(err error) string {
	switch {
	case errors.Is(err, fs.ErrNotExist):
		return "no such file or directory"
	case errors.Is(err, fs.ErrPermission):
		return "permission denied"
	}
	var pathErr *fs.PathError
	if errors.As(err, &pathErr) {
		return pathErr.Err.Error()
	}
	return err.Error()
}

// readLines reads the lines of a file the model sees: those from start to
// end, from start on, or the file's beginning, within budget bytes; and
// counts the file's lines.
func readLines(path string, file *contextbuilder.LinkedFile, start, end, budget int) {
	f, err := os.Open(path)
	if err != nil {
		file.Error = describeError(err)
		return
	}
	defer f.Close()
	reader := bufio.NewReaderSize(io.LimitReader(f, scanBytes), 64<<10)
	if head, _ := reader.Peek(sniffBytes); bytes.IndexByte(head, 0) >= 0 {
		file.Binary = true
		return
	}
	// The lines to show, and the bytes they may take.
	whole := start == 0 && end == 0 && file.Size <= wholeBytes
	from, to, limit := 1, previewLines, previewBytes
	switch {
	case whole:
		to, limit = wholeLines, wholeBytes
	case start > 0 && end == 0:
		from, to = start, start+previewLines-1
	case end > 0:
		from = max(1, start)
		to, limit = min(end, from+rangeLines-1), rangeBytes
	}
	limit = min(limit, budget)
	counting := file.Size <= countBytes
	var shown []string
	size, n := 0, 0
	full := true // every line from `from` on fit
	for {
		line, ok, err := nextLine(reader)
		if err != nil {
			file.Error = describeError(err)
			return
		}
		if !ok {
			break
		}
		n++
		if n >= from && n <= to && full {
			if size+len(line)+1 > limit {
				full = false
			} else {
				shown = append(shown, line)
				size += len(line) + 1
			}
		}
		if n >= to && !counting {
			break
		}
	}
	if counting {
		file.Lines = n
	}
	if whole && (n > wholeLines || !full) {
		// Too many lines to show whole: the beginning shows.
		shown = fit(shown, previewLines, min(previewBytes, budget))
	}
	if n < from && !counting && len(shown) == 0 {
		file.Error = fmt.Sprintf("line %d is past the first %d MB, which are all that are read", from, scanBytes>>20)
		return
	}
	if len(shown) != 0 {
		file.From, file.To = from, from+len(shown)-1
		file.Content = strings.Join(shown, "\n")
	}
}

// fit keeps the first lines that fit count lines and limit bytes.
func fit(lines []string, count, limit int) []string {
	size := 0
	for n, line := range lines {
		if n == count || size+len(line)+1 > limit {
			return lines[:n]
		}
		size += len(line) + 1
	}
	return lines
}

// nextLine reads a line without its line break, as the model sees it:
// valid UTF-8, and cut short past lineBytes. ok is false at the end.
func nextLine(reader *bufio.Reader) (line string, ok bool, err error) {
	// kept holds the line's first lineBytes+1 bytes: enough to tell a line
	// too long to show whole.
	var kept []byte
	length, ended := 0, false
	for {
		chunk, err := reader.ReadSlice('\n')
		length += len(chunk)
		if room := lineBytes + 1 - len(kept); room > 0 {
			kept = append(kept, chunk[:min(len(chunk), room)]...)
		}
		if errors.Is(err, bufio.ErrBufferFull) {
			continue
		}
		if err != nil && !errors.Is(err, io.EOF) {
			return "", false, err
		}
		ended = err == nil
		break
	}
	if length == 0 {
		return "", false, nil
	}
	content := length
	if ended {
		content-- // the line break
	}
	if content <= lineBytes {
		return strings.ToValidUTF8(string(bytes.TrimSuffix(kept[:content], []byte("\r"))), "\uFFFD"), true, nil
	}
	cut := lineBytes
	for cut > 0 && !utf8.RuneStart(kept[cut]) {
		cut--
	}
	return strings.ToValidUTF8(string(kept[:cut]), "\uFFFD") + fmt.Sprintf(" […%d more bytes]", content-cut), true, nil
}

// listFolder lists the entries of a folder the model sees: its folders
// first, each with a slash at the end, then its files, within budget bytes.
func listFolder(path string, file *contextbuilder.LinkedFile, budget int) {
	entries, err := os.ReadDir(path)
	if err != nil && len(entries) == 0 {
		file.Error = describeError(err)
		return
	}
	type entry struct {
		name   string
		folder bool
	}
	var listed []entry
	for _, one := range entries {
		name := one.Name()
		if name == ".git" || name == ".DS_Store" {
			continue
		}
		folder := one.IsDir()
		if one.Type()&fs.ModeSymlink != 0 {
			if info, err := os.Stat(filepath.Join(path, name)); err == nil {
				folder = info.IsDir()
			}
		}
		listed = append(listed, entry{strings.ToValidUTF8(name, "\uFFFD"), folder})
	}
	slices.SortStableFunc(listed, func(a, b entry) int {
		if a.folder != b.folder {
			if a.folder {
				return -1
			}
			return 1
		}
		return cmp.Compare(strings.ToLower(a.name), strings.ToLower(b.name))
	})
	names := make([]string, len(listed))
	for n, one := range listed {
		names[n] = one.name
		if one.folder {
			names[n] += "/"
		}
	}
	file.Lines = len(names)
	if shown := fit(names, folderEntries, min(rangeBytes, budget)); len(shown) != 0 {
		file.From, file.To = 1, len(shown)
		file.Content = strings.Join(shown, "\n")
	}
}
