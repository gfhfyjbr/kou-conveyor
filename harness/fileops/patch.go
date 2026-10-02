package fileops

import (
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
)

// The apply_patch format, as the GPT models write it:
//
//	*** Begin Patch
//	*** Add File: path/to/new.go
//	+package main
//	*** Delete File: path/to/old.go
//	*** Update File: path/to/changed.go
//	*** Move to: path/to/renamed.go
//	@@ func Example()
//	 context line
//	-removed line
//	+added line
//	*** End of File
//	*** End Patch
//
// An update holds sections, each started by an @@ line (with an optional
// context header naming a line to find first, such as a function's
// signature) or by nothing at all for the first one, and made of context
// lines (a space first), removed lines (-) and added lines (+). The context
// and the removed lines locate the section in the file: exactly, then
// without trailing whitespace, then without any; *** End of File says the
// section reaches the file's end.

// PatchResult says what a patch did, by file.
type PatchResult struct {
	Added    []string
	Modified []string
	Deleted  []string
	// Moved lists renames as "old -> new".
	Moved []string
}

// Summary is a line per file, for the model.
func (result PatchResult) Summary() string {
	var lines []string
	for _, name := range result.Added {
		lines = append(lines, "A "+name)
	}
	for _, name := range result.Modified {
		lines = append(lines, "M "+name)
	}
	for _, name := range result.Moved {
		lines = append(lines, "R "+name)
	}
	for _, name := range result.Deleted {
		lines = append(lines, "D "+name)
	}
	if len(lines) == 0 {
		return "The patch changed nothing."
	}
	return "Applied the patch:\n" + strings.Join(lines, "\n")
}

// Files lists every path the patch touched, for the cockpits.
func (result PatchResult) Files() []string {
	files := append(append(append([]string{}, result.Added...), result.Modified...), result.Deleted...)
	for _, moved := range result.Moved {
		if _, to, ok := strings.Cut(moved, " -> "); ok {
			files = append(files, to)
		}
	}
	return files
}

type patchKind int

const (
	patchAdd patchKind = iota + 1
	patchDelete
	patchUpdate
)

type patchFile struct {
	kind     patchKind
	path     string
	moveTo   string
	lines    []string // an added file's lines
	sections []patchSection
}

type patchSection struct {
	// context is the line an @@ header names, which the section follows.
	context []string
	// old are the lines the section replaces (context and removed lines);
	// chunks say what changes within them.
	old    []string
	chunks []patchChunk
	eof    bool
}

// A chunk is a run of removed and added lines at offset in the section's
// old lines.
type patchChunk struct {
	offset  int
	removed []string
	added   []string
}

// ApplyPatch applies a patch in the apply_patch format. Relative paths are
// resolved against root. Nothing is written until the whole patch parsed
// and every section was found, so a patch that fails leaves the files as
// they were.
func ApplyPatch(patch, root string) (PatchResult, error) {
	files, err := parsePatch(patch)
	if err != nil {
		return PatchResult{}, err
	}
	if len(files) == 0 {
		return PatchResult{}, errors.New("the patch holds no file")
	}
	var result PatchResult
	resolve := func(name string) string {
		if filepath.IsAbs(name) {
			return filepath.Clean(name)
		}
		return filepath.Join(root, name)
	}
	// The patch works on a view of the files it touches, so that a section
	// sees what the ones before it did (a file the patch added, or updated
	// already); the view is written once every section applied.
	type file struct {
		exists  bool        // in the view
		info    os.FileInfo // on disk; nil when it is not there
		statErr error
		content string
		loaded  bool
		mode    os.FileMode
		write   bool
	}
	view := map[string]*file{}
	var touched []string
	get := func(target string) *file {
		if current, ok := view[target]; ok {
			return current
		}
		current := &file{mode: 0o644}
		if info, err := os.Stat(target); err == nil {
			current.exists, current.info, current.mode = true, info, info.Mode().Perm()
		} else {
			current.statErr = err
		}
		view[target] = current
		touched = append(touched, target)
		return current
	}
	missing := func(current *file, name string) error {
		if current.statErr != nil {
			return describe(current.statErr, name)
		}
		return fmt.Errorf("%s does not exist", name)
	}
	for _, change := range files {
		target := resolve(change.path)
		current := get(target)
		directory := current.info != nil && current.info.IsDir() && !current.loaded
		switch change.kind {
		case patchAdd:
			if current.exists {
				return PatchResult{}, fmt.Errorf("cannot add %s: it exists; use Update File to change it", change.path)
			}
			content := strings.Join(change.lines, "\n")
			if len(change.lines) != 0 {
				content += "\n"
			}
			current.exists, current.content, current.loaded, current.write = true, content, true, true
			result.Added = append(result.Added, change.path)
		case patchDelete:
			if !current.exists {
				return PatchResult{}, fmt.Errorf("cannot delete %s: %v", change.path, missing(current, change.path))
			}
			if directory {
				return PatchResult{}, fmt.Errorf("cannot delete %s: it is a directory", change.path)
			}
			current.exists, current.write = false, false
			result.Deleted = append(result.Deleted, change.path)
		case patchUpdate:
			if !current.exists {
				return PatchResult{}, fmt.Errorf("cannot update %s: %v", change.path, missing(current, change.path))
			}
			if directory {
				return PatchResult{}, fmt.Errorf("cannot update %s: it is a directory", change.path)
			}
			if !current.loaded {
				if current.info.Size() > MaxEditBytes {
					return PatchResult{}, fmt.Errorf("cannot update %s: %s is too large", change.path, formatBytes(current.info.Size()))
				}
				data, err := os.ReadFile(target)
				if err != nil {
					return PatchResult{}, describe(err, change.path)
				}
				current.content, current.loaded = string(data), true
			}
			updated, err := applySections(current.content, change.sections)
			if err != nil {
				return PatchResult{}, fmt.Errorf("%s: %w", change.path, err)
			}
			if change.moveTo == "" {
				current.content, current.write = updated, true
				if !slices.Contains(result.Added, change.path) && !slices.Contains(result.Modified, change.path) {
					result.Modified = append(result.Modified, change.path)
				}
				continue
			}
			destination := resolve(change.moveTo)
			moved := get(destination)
			if destination != target {
				if moved.exists {
					return PatchResult{}, fmt.Errorf("cannot move %s to %s: it exists", change.path, change.moveTo)
				}
				current.exists, current.write = false, false
			}
			moved.exists, moved.content, moved.loaded, moved.write, moved.mode = true, updated, true, true, current.mode
			result.Moved = append(result.Moved, change.path+" -> "+change.moveTo)
		}
	}
	// What the patch made is written first, then what it deleted or moved
	// away is removed.
	for _, target := range touched {
		current := view[target]
		if !current.exists || !current.write {
			continue
		}
		if err := os.MkdirAll(filepath.Dir(target), 0o755); err != nil {
			return PatchResult{}, fmt.Errorf("create the directory of %s: %w", target, err)
		}
		if err := replaceFile(target, []byte(current.content), current.mode); err != nil {
			return PatchResult{}, err
		}
	}
	for _, target := range touched {
		if current := view[target]; !current.exists && current.info != nil {
			if err := os.Remove(target); err != nil && !errors.Is(err, os.ErrNotExist) {
				return PatchResult{}, fmt.Errorf("delete %s: %w", target, err)
			}
		}
	}
	return result, nil
}

// parsePatch reads the files of a patch.
func parsePatch(patch string) ([]patchFile, error) {
	lines := strings.Split(strings.ReplaceAll(patch, "\r\n", "\n"), "\n")
	// The patch may come with text around it; the markers bound it.
	start := -1
	for index, line := range lines {
		if strings.TrimSpace(line) == "*** Begin Patch" {
			start = index
			break
		}
	}
	if start < 0 {
		return nil, errors.New("the patch must start with *** Begin Patch")
	}
	end := -1
	for index := len(lines) - 1; index > start; index-- {
		if strings.TrimSpace(lines[index]) == "*** End Patch" {
			end = index
			break
		}
	}
	if end < 0 {
		return nil, errors.New("the patch must end with *** End Patch")
	}
	lines = lines[start+1 : end]
	var files []patchFile
	index := 0
	for index < len(lines) {
		line := lines[index]
		switch {
		case strings.HasPrefix(line, "*** Add File: "):
			file := patchFile{kind: patchAdd, path: strings.TrimSpace(strings.TrimPrefix(line, "*** Add File: "))}
			index++
			for index < len(lines) && !strings.HasPrefix(lines[index], "*** ") {
				added := lines[index]
				if !strings.HasPrefix(added, "+") {
					return nil, fmt.Errorf("line %d of the added file %s must start with +: %q", index+1, file.path, added)
				}
				file.lines = append(file.lines, added[1:])
				index++
			}
			files = append(files, file)
		case strings.HasPrefix(line, "*** Delete File: "):
			files = append(files, patchFile{kind: patchDelete, path: strings.TrimSpace(strings.TrimPrefix(line, "*** Delete File: "))})
			index++
		case strings.HasPrefix(line, "*** Update File: "):
			file := patchFile{kind: patchUpdate, path: strings.TrimSpace(strings.TrimPrefix(line, "*** Update File: "))}
			index++
			if index < len(lines) && strings.HasPrefix(lines[index], "*** Move to: ") {
				file.moveTo = strings.TrimSpace(strings.TrimPrefix(lines[index], "*** Move to: "))
				index++
			}
			var err error
			file.sections, index, err = parseSections(lines, index, file.path)
			if err != nil {
				return nil, err
			}
			if len(file.sections) == 0 && file.moveTo == "" {
				return nil, fmt.Errorf("the update of %s changes nothing", file.path)
			}
			files = append(files, file)
		case strings.TrimSpace(line) == "":
			index++
		default:
			return nil, fmt.Errorf("line %d: expected *** Add File:, *** Update File: or *** Delete File:, got %q", index+1, line)
		}
		if file := files[len(files)-1]; file.path == "" {
			return nil, errors.New("a file line names no path")
		}
	}
	return files, nil
}

// parseSections reads the sections of an update, up to the next file.
func parseSections(lines []string, index int, path string) ([]patchSection, int, error) {
	var sections []patchSection
	for index < len(lines) {
		line := lines[index]
		if strings.HasPrefix(line, "*** ") && !strings.HasPrefix(line, "*** End of File") {
			break
		}
		section := patchSection{}
		if strings.HasPrefix(line, "@@") {
			// Nested headers stack: each names a line to find after the last.
			for index < len(lines) && strings.HasPrefix(lines[index], "@@") {
				if header := strings.TrimSpace(strings.TrimPrefix(lines[index], "@@")); header != "" {
					section.context = append(section.context, header)
				}
				index++
			}
		} else if len(sections) != 0 {
			return nil, 0, fmt.Errorf("%s: line %d: a section after the first must start with @@", path, index+1)
		}
		chunk := (*patchChunk)(nil)
		for index < len(lines) {
			line := lines[index]
			if strings.HasPrefix(line, "@@") || strings.HasPrefix(line, "*** ") && !strings.HasPrefix(line, "*** End of File") {
				break
			}
			index++
			switch {
			case line == "*** End of File":
				section.eof = true
			case strings.HasPrefix(line, "+"):
				if chunk == nil {
					section.chunks = append(section.chunks, patchChunk{offset: len(section.old)})
					chunk = &section.chunks[len(section.chunks)-1]
				}
				chunk.added = append(chunk.added, line[1:])
			case strings.HasPrefix(line, "-"):
				if chunk == nil {
					section.chunks = append(section.chunks, patchChunk{offset: len(section.old)})
					chunk = &section.chunks[len(section.chunks)-1]
				}
				chunk.removed = append(chunk.removed, line[1:])
				section.old = append(section.old, line[1:])
			default:
				// A context line, with its leading space or without one.
				chunk = nil
				section.old = append(section.old, strings.TrimPrefix(line, " "))
			}
		}
		if len(section.chunks) == 0 {
			// Blank lines where a section could start, before the first @@
			// say, are no section.
			if len(section.context) == 0 && strings.TrimSpace(strings.Join(section.old, "")) == "" {
				continue
			}
			return nil, 0, fmt.Errorf("%s: a section adds or removes nothing", path)
		}
		sections = append(sections, section)
	}
	return sections, index, nil
}

// applySections applies the sections of an update to content.
func applySections(content string, sections []patchSection) (string, error) {
	// A file with Windows line endings is patched in lines, and keeps them.
	windows := strings.Count(content, "\r\n") > strings.Count(content, "\n")/2
	if windows {
		content = strings.ReplaceAll(content, "\r\n", "\n")
	}
	lines := strings.Split(content, "\n")
	trailingNewline := strings.HasSuffix(content, "\n")
	if trailingNewline {
		lines = lines[:len(lines)-1]
	}
	cursor := 0
	type edit struct {
		at      int
		remove  int
		insert  []string
		section int
	}
	var edits []edit
	for number, section := range sections {
		for _, header := range section.context {
			at := findLine(lines, header, cursor)
			if at < 0 {
				return "", fmt.Errorf("the @@ context %q of section %d was not found after line %d", header, number+1, cursor)
			}
			cursor = at + 1
		}
		at, err := findContext(lines, section.old, cursor, section.eof)
		if err != nil {
			// A patch often ends a section with a blank line the file does
			// not have there.
			if trimmed := withoutTrailingBlank(section); len(trimmed.old) < len(section.old) {
				if found, retry := findContext(lines, trimmed.old, cursor, trimmed.eof); retry == nil {
					section, at, err = trimmed, found, nil
				}
			}
		}
		if err != nil {
			return "", fmt.Errorf("section %d: %w", number+1, err)
		}
		for _, chunk := range section.chunks {
			edits = append(edits, edit{at: at + chunk.offset, remove: len(chunk.removed), insert: chunk.added, section: number})
		}
		cursor = at + len(section.old)
	}
	var updated []string
	position := 0
	for _, change := range edits {
		if change.at < position {
			return "", fmt.Errorf("section %d overlaps the one before it", change.section+1)
		}
		updated = append(updated, lines[position:change.at]...)
		updated = append(updated, change.insert...)
		position = change.at + change.remove
	}
	updated = append(updated, lines[position:]...)
	result := strings.Join(updated, "\n")
	if trailingNewline || len(updated) > 0 && result != "" {
		result += "\n"
	}
	if windows {
		result = strings.ReplaceAll(result, "\n", "\r\n")
	}
	return result, nil
}

// withoutTrailingBlank is a section without the blank context lines that
// end it.
func withoutTrailingBlank(section patchSection) patchSection {
	end := 0
	for _, chunk := range section.chunks {
		end = max(end, chunk.offset+len(chunk.removed))
	}
	old := section.old
	for len(old) > end && strings.TrimSpace(old[len(old)-1]) == "" {
		old = old[:len(old)-1]
	}
	section.old = old
	return section
}

// findLine finds a line equal to text at or after from: exactly, then
// ignoring surrounding whitespace.
func findLine(lines []string, text string, from int) int {
	for index := from; index < len(lines); index++ {
		if lines[index] == text {
			return index
		}
	}
	trimmed := strings.TrimSpace(text)
	for index := from; index < len(lines); index++ {
		if strings.TrimSpace(lines[index]) == trimmed {
			return index
		}
	}
	return -1
}

// findContext finds where old begins in lines, at or after from: with eof
// set, at the end of the file first. Lines are compared exactly, then
// without trailing whitespace, then without any around them.
func findContext(lines, old []string, from int, eof bool) (int, error) {
	if len(old) == 0 {
		if eof {
			return len(lines), nil
		}
		return from, nil
	}
	compare := []func(a, b string) bool{
		func(a, b string) bool { return a == b },
		func(a, b string) bool { return strings.TrimRight(a, " \t") == strings.TrimRight(b, " \t") },
		func(a, b string) bool { return strings.TrimSpace(a) == strings.TrimSpace(b) },
	}
	matches := func(at int, equal func(a, b string) bool) bool {
		if at < 0 || at+len(old) > len(lines) {
			return false
		}
		for offset, want := range old {
			if !equal(lines[at+offset], want) {
				return false
			}
		}
		return true
	}
	for _, equal := range compare {
		if eof && matches(len(lines)-len(old), equal) {
			return len(lines) - len(old), nil
		}
		for at := from; at+len(old) <= len(lines); at++ {
			if matches(at, equal) {
				return at, nil
			}
		}
	}
	shown := old
	if len(shown) > 3 {
		shown = shown[:3]
	}
	return 0, fmt.Errorf("its context was not found after line %d; the file does not hold these lines as the patch gives them:\n%s\nRead the file again and copy the lines exactly", from, strings.Join(shown, "\n"))
}
