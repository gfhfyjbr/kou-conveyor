package cockpit

import (
	"bytes"
	"cmp"
	"context"
	"errors"
	"io/fs"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"
	"unicode"
	"unicode/utf8"
)

// File links. A prompt links a file or a folder by a $ reference in its
// text: "$cmd/main.go", "$cmd/main.go:120" (from line 120 on),
// "$cmd/main.go:120-160" (those lines), "$cmd/" (a folder's entries),
// "$~/notes.md", "$/etc/hosts", and "$\"a b.txt\"" for a path with spaces.
// A $ starts a reference at the start of the text, after a space, a comma
// or an opening bracket or quote, outside code spans and fenced code, and
// not after a backslash; a reference names a file or folder that exists,
// or it is no link: "$HOME", "$1" and "$(pwd)" stay text. Punctuation that
// ends a sentence is no part of a path, unless the path with it exists.
//
// The runner reads what the model sees of each link as it takes the prompt:
// the lines asked for, else the beginning of the file, or a folder's
// entries; the session records it with the prompt, and the model reads the
// rest from the file itself. Composers complete a $ to the workspace's
// files and folders (FileIndex, MatchFiles).

// MaxFileLinks bounds the links of one prompt; the runner takes no more.
const MaxFileLinks = 20

// maxReference bounds the bytes of a reference.
const maxReference = 1024

// FileLink is a file or a folder a prompt links to, as the runner takes it.
type FileLink struct {
	// Label is the reference as the text has it: "$cmd/main.go:10-40".
	Label string `json:"label"`
	// Path is the path it names: relative to the workspace, absolute, or
	// in the home folder (~/).
	Path string `json:"path"`
	// StartLine and EndLine are the lines it asks for; 0 where it does not.
	StartLine int `json:"start_line,omitzero"`
	EndLine   int `json:"end_line,omitzero"`
	// Directory and Size say what it names, for composers to show.
	Directory bool  `json:"-"`
	Size      int64 `json:"-"`
}

// FileLinks returns the links of text: its references to files and folders
// that exist, each label once, in the order of the text, and at most
// MaxFileLinks of them.
func FileLinks(text, workspace string) []FileLink {
	return fileLinks(text, func(name string) (fs.FileInfo, error) { return os.Stat(LinkTarget(workspace, name)) })
}

func fileLinks(text string, stat func(string) (fs.FileInfo, error)) []FileLink {
	var links []FileLink
	for _, ref := range references(text) {
		link, ok := ref.resolve(stat)
		if !ok || slices.ContainsFunc(links, func(other FileLink) bool { return other.Label == link.Label }) {
			continue
		}
		links = append(links, link)
		if len(links) == MaxFileLinks {
			break
		}
	}
	return links
}

// LinkTarget is where a linked path is: in the workspace, unless it is
// absolute or in the home folder.
func LinkTarget(workspace, name string) string {
	if rest, ok := strings.CutPrefix(name, "~/"); ok || name == "~" {
		if home, err := os.UserHomeDir(); err == nil {
			return filepath.Join(home, rest)
		}
	}
	if filepath.IsAbs(name) {
		return filepath.Clean(name)
	}
	return filepath.Join(workspace, name)
}

// LinkLabel is how a prompt links a path: "$cmd/main.go", or quoted where
// the path has spaces.
func LinkLabel(name string) string {
	if strings.IndexFunc(name, unicode.IsSpace) >= 0 && !strings.Contains(name, `"`) {
		return `$"` + name + `"`
	}
	return "$" + name
}

// reference is a $ of a text that may start a link: raw is the text from
// the $ on that it may take, or its quoted path and what follows the quote.
type reference struct {
	raw    string
	quoted bool
}

// references finds the places in text where a link may start.
func references(text string) []reference {
	code := codeRanges(text)
	var refs []reference
	for at := 0; at < len(text); {
		next := strings.IndexByte(text[at:], '$')
		if next < 0 {
			break
		}
		start := at + next
		at = start + 1
		if !opensReference(text, start) || inRanges(code, start) {
			continue
		}
		rest := text[start+1:]
		if quoted, ok := strings.CutPrefix(rest, `"`); ok {
			end := strings.IndexAny(quoted, "\"\n")
			if end <= 0 || quoted[end] != '"' || end > maxReference {
				continue
			}
			// The quoted path, and what follows it up to a space.
			after := quoted[end+1:]
			if space := strings.IndexFunc(after, unicode.IsSpace); space >= 0 {
				after = after[:space]
			}
			refs = append(refs, reference{raw: quoted[:end+1] + after, quoted: true})
			continue
		}
		end := strings.IndexFunc(rest, unicode.IsSpace)
		if end < 0 {
			end = len(rest)
		}
		if end == 0 || end > maxReference {
			continue
		}
		refs = append(refs, reference{raw: rest[:end]})
	}
	return refs
}

// opensReference reports whether the $ at i may start a reference: it
// follows the start of the text, a space, a comma, or an opening bracket
// or quote, and no backslash escapes it.
func opensReference(text string, i int) bool {
	if i == 0 {
		return true
	}
	before, _ := utf8.DecodeLastRuneInString(text[:i])
	return unicode.IsSpace(before) || strings.ContainsRune(`([{<"',`, before)
}

// trailing is punctuation that ends a sentence or a clause after a path.
const trailing = `.,;:!?)]}>'"…»`

var lineSuffix = regexp.MustCompile(`^:([0-9]{1,9})(?:-([0-9]{1,9}))?$`)

// resolve makes a reference a link to what it names, if that exists.
func (ref reference) resolve(stat func(string) (fs.FileInfo, error)) (FileLink, bool) {
	if ref.quoted {
		end := strings.IndexByte(ref.raw, '"')
		name, after := ref.raw[:end], ref.raw[end+1:]
		for {
			if link, ok := linkTo(name, after, stat); ok {
				link.Label = `$"` + name + `"` + after
				return link, true
			}
			if after == "" {
				return FileLink{}, false
			}
			r, size := utf8.DecodeLastRuneInString(after)
			if !strings.ContainsRune(trailing, r) {
				return FileLink{}, false
			}
			after = after[:len(after)-size]
		}
	}
	token := ref.raw
	for token != "" {
		if link, ok := linkTo(token, "", stat); ok {
			link.Label = "$" + token
			return link, true
		}
		if at := strings.LastIndexByte(token, ':'); at > 0 {
			if link, ok := linkTo(token[:at], token[at:], stat); ok {
				link.Label = "$" + token
				return link, true
			}
		}
		r, size := utf8.DecodeLastRuneInString(token)
		if !strings.ContainsRune(trailing, r) {
			break
		}
		token = token[:len(token)-size]
	}
	return FileLink{}, false
}

// linkTo links name, with the lines a suffix such as ":10-40" asks for,
// if name is a file or folder that exists.
func linkTo(name, suffix string, stat func(string) (fs.FileInfo, error)) (FileLink, bool) {
	if strings.Trim(name, "./~") == "" || strings.ContainsAny(name, "\x00\n\r") {
		return FileLink{}, false
	}
	link := FileLink{Path: name}
	if suffix != "" {
		match := lineSuffix.FindStringSubmatch(suffix)
		if match == nil {
			return FileLink{}, false
		}
		link.StartLine, _ = strconv.Atoi(match[1])
		if match[2] != "" {
			link.EndLine, _ = strconv.Atoi(match[2])
		}
		if link.StartLine < 1 || link.EndLine != 0 && link.EndLine < link.StartLine {
			return FileLink{}, false
		}
	}
	info, err := stat(name)
	if err != nil || suffix != "" && info.IsDir() {
		return FileLink{}, false
	}
	link.Directory = info.IsDir()
	if !link.Directory {
		link.Size = info.Size()
	}
	return link, true
}

// codeRanges returns where text has code: fenced blocks and code spans, as
// byte ranges [from, to).
func codeRanges(text string) [][2]int {
	var ranges [][2]int
	// Fenced blocks: a line of three or more backticks or tildes opens one,
	// which a line of as many of the same closes, or the end of the text.
	fence, fenceStart := "", 0
	offset := 0
	for line := range strings.Lines(text) {
		trimmed := strings.TrimLeft(line, " ")
		if len(line)-len(trimmed) <= 3 {
			marker := fenceMarker(trimmed)
			switch {
			case fence == "" && marker != "":
				fence, fenceStart = marker, offset
			case fence != "" && strings.HasPrefix(marker, fence) && strings.TrimSpace(trimmed[len(marker):]) == "":
				ranges = append(ranges, [2]int{fenceStart, offset + len(line)})
				fence = ""
			}
		}
		offset += len(line)
	}
	if fence != "" {
		ranges = append(ranges, [2]int{fenceStart, len(text)})
	}
	// Code spans: a run of backticks, up to the next run of as many.
	for at := 0; at < len(text); {
		open := strings.IndexByte(text[at:], '`')
		if open < 0 {
			break
		}
		open += at
		if inRanges(ranges, open) {
			at = open + 1
			continue
		}
		run := len(text[open:]) - len(strings.TrimLeft(text[open:], "`"))
		closed := false
		for search := open + run; search < len(text); {
			next := strings.IndexByte(text[search:], '`')
			if next < 0 {
				break
			}
			next += search
			length := len(text[next:]) - len(strings.TrimLeft(text[next:], "`"))
			if length == run {
				ranges = append(ranges, [2]int{open, next + length})
				at, closed = next+length, true
				break
			}
			search = next + length
		}
		if !closed {
			at = open + run
		}
	}
	return ranges
}

// fenceMarker returns the backticks or tildes that start line, three or
// more, or "".
func fenceMarker(line string) string {
	for _, char := range []string{"`", "~"} {
		marker := line[:len(line)-len(strings.TrimLeft(line, char))]
		if len(marker) >= 3 {
			return marker
		}
	}
	return ""
}

func inRanges(ranges [][2]int, i int) bool {
	for _, r := range ranges {
		if i >= r[0] && i < r[1] {
			return true
		}
	}
	return false
}

// ---------------------------------------------------------------- completing

// LinkQuery finds the reference being typed at the end of before, the text
// before the cursor: where its $ is, the path typed so far, and whether it
// is quoted. A reference that has lines typed after its path completes no
// more.
func LinkQuery(before string) (start int, query string, quoted, ok bool) {
	code := codeRanges(before)
	// A quoted path still open, which may hold spaces: $"a b
	if at := strings.LastIndex(before, `$"`); at >= 0 && opensReference(before, at) && !inRanges(code, at) {
		if name := before[at+2:]; !strings.ContainsAny(name, "\"\n") && len(name) <= maxReference {
			return at, name, true, true
		}
	}
	// Otherwise the last word, from its first $ that opens a reference.
	word := strings.LastIndexFunc(before, unicode.IsSpace) + 1
	for at := word; at < len(before); at++ {
		if before[at] != '$' || !opensReference(before, at) || inRanges(code, at) {
			continue
		}
		rest := before[at+1:]
		if len(rest) > maxReference || strings.HasPrefix(rest, `"`) {
			return 0, "", false, false
		}
		if colon := strings.LastIndexByte(rest, ':'); colon >= 0 && colon < len(rest)-1 && strings.Trim(rest[colon+1:], "0123456789-") == "" {
			return 0, "", false, false // lines are being typed
		}
		return at, rest, false, true
	}
	return 0, "", false, false
}

// Complete lists what a $ reference that has query so far can complete
// to, the best first and at most limit of them: the workspace's files and
// folders (MatchFiles), or, for a path from / or ~/, what is in the folder
// it names.
func (x *FileIndex) Complete(ctx context.Context, query string, limit int) ([]FileMatch, error) {
	if strings.HasPrefix(query, "/") || strings.HasPrefix(query, "~/") {
		return completePath(query, limit), nil
	}
	entries, err := x.Entries(ctx)
	return MatchFiles(entries, query, limit), err
}

// completePath lists what is in the folder a path typed from / or ~/ ends
// in, whose names start with what follows its last slash, written the way
// it was typed: folders first, each with a slash at the end. Hidden names
// show once a dot is typed.
func completePath(typed string, limit int) []FileMatch {
	cut := strings.LastIndexByte(typed, '/') + 1
	dir, prefix := typed[:cut], strings.ToLower(typed[cut:])
	entries, err := os.ReadDir(LinkTarget("", dir))
	if err != nil {
		return nil
	}
	var matches []FileMatch
	for _, entry := range entries {
		name := entry.Name()
		if !strings.HasPrefix(strings.ToLower(name), prefix) || strings.HasPrefix(name, ".") && !strings.HasPrefix(prefix, ".") {
			continue
		}
		folder := entry.IsDir()
		if entry.Type()&fs.ModeSymlink != 0 {
			if info, err := os.Stat(filepath.Join(LinkTarget("", dir), name)); err == nil {
				folder = info.IsDir()
			}
		}
		match := FileMatch{Path: dir + name, Directory: folder}
		if folder {
			match.Path += "/"
		}
		matches = append(matches, match)
	}
	slices.SortFunc(matches, func(a, b FileMatch) int {
		if a.Directory != b.Directory {
			if a.Directory {
				return -1
			}
			return 1
		}
		return cmp.Compare(strings.ToLower(a.Path), strings.ToLower(b.Path))
	})
	return matches[:min(len(matches), limit)]
}

// FileMatch is a file or a folder a $ reference being typed can complete
// to. A folder's path ends in a slash.
type FileMatch struct {
	Path      string `json:"path"`
	Directory bool   `json:"directory,omitzero"`
}

// MatchFiles returns the entries a reference that has query so far can
// complete to, the best first, at most limit of them. Nothing typed lists
// the workspace's top level, and a folder typed with its slash its
// entries; otherwise paths that start with the query come first, then
// names that start with it, then paths that hold it, and then those that
// hold its letters in order, shorter paths before longer ones.
func MatchFiles(entries []string, query string, limit int) []FileMatch {
	type scored struct {
		entry string
		score int
	}
	var found []scored
	lower := strings.ToLower(query)
	folder := strings.HasSuffix(query, "/") && slices.Contains(entries, query)
	for _, entry := range entries {
		switch {
		case query == "" || folder:
			// The entries right inside the folder, or the top level.
			rest, ok := strings.CutPrefix(entry, query)
			if ok && rest != "" && !strings.Contains(strings.TrimSuffix(rest, "/"), "/") {
				score := 2
				if strings.HasSuffix(rest, "/") {
					score = 3
				}
				found = append(found, scored{entry, score})
			}
		default:
			if score := matchScore(strings.ToLower(entry), lower); score > 0 {
				found = append(found, scored{entry, score})
			}
		}
	}
	slices.SortFunc(found, func(a, b scored) int {
		return cmp.Or(cmp.Compare(b.score, a.score), cmp.Compare(len(a.entry), len(b.entry)), cmp.Compare(a.entry, b.entry))
	})
	matches := make([]FileMatch, 0, min(len(found), limit))
	for _, one := range found[:min(len(found), limit)] {
		matches = append(matches, FileMatch{Path: one.entry, Directory: strings.HasSuffix(one.entry, "/")})
	}
	return matches
}

// matchScore scores how well an entry matches a query, both lower case: 0
// for not at all.
func matchScore(entry, query string) int {
	name := path.Base(strings.TrimSuffix(entry, "/"))
	switch {
	case strings.HasPrefix(entry, query):
		return 5000
	case strings.HasPrefix(name, query):
		return 4000
	}
	if at := strings.Index(name, query); at >= 0 {
		return 3000 - min(at, 100)
	}
	if at := strings.Index(entry, query); at >= 0 {
		bonus := 0
		if at == 0 || strings.ContainsRune("/._-", rune(entry[at-1])) {
			bonus = 200
		}
		return 2000 + bonus - min(at, 100)
	}
	// The query's letters in order, close together: fewer gaps and more at
	// the starts of words better. Letters strewn along a long path match
	// nothing that was meant.
	score, from, first, last := 1000, 0, -1, -1
	for _, r := range query {
		next := strings.IndexRune(entry[from:], r)
		if next < 0 {
			return 0
		}
		next += from
		if first < 0 {
			first = next
		}
		if last >= 0 {
			score -= min(next-last-1, 20)
		}
		if next == 0 || strings.ContainsRune("/._-", rune(entry[next-1])) {
			score += 10
		}
		last, from = next, next+utf8.RuneLen(r)
	}
	if span := last - first + 1; span > max(3*len(query), len(query)+8) {
		return 0
	}
	return max(1, min(score, 1999))
}

// ---------------------------------------------------------------- the index

// FileIndex lists the files and folders of a workspace for composers to
// complete links with. Completing asks for the list at every key, so it is
// kept, and listed anew in the background once it is a few seconds old —
// longer where listing takes long: a large folder without git.
type FileIndex struct {
	workspace string
	// skip leaves out the harness's own files in the workspace: sessions
	// and the runner's logs.
	skip func(name string) bool

	mu         sync.Mutex
	entries    []string
	err        error
	at         time.Time
	took       time.Duration
	refreshing bool
}

const (
	// fileIndexFresh is how long a FileIndex keeps a list at least.
	fileIndexFresh = 3 * time.Second
	// maxIndexed bounds the files it lists, and indexTime how long it
	// lists them for.
	maxIndexed = 200_000
	indexTime  = 5 * time.Second
)

// NewFileIndex lists the files of workspace when it is first asked to,
// leaving out those of sessionDir and the runner's logs in logDir ("" for
// its default, logs in the workspace), where the workspace holds them.
func NewFileIndex(workspace, sessionDir, logDir string) *FileIndex {
	if logDir == "" {
		logDir = filepath.Join(workspace, "logs")
	}
	inside := func(dir string) string {
		rel, err := filepath.Rel(workspace, dir)
		if err != nil || rel == "." || rel == ".." || strings.HasPrefix(rel, "../") || filepath.IsAbs(rel) {
			return ""
		}
		return filepath.ToSlash(rel) + "/"
	}
	sessions, logs := inside(sessionDir), inside(logDir)
	return &FileIndex{workspace: workspace, skip: func(name string) bool {
		if sessions != "" && strings.HasPrefix(name, sessions) {
			return true
		}
		// The runner's logs, not whatever else is there.
		rest, ok := strings.CutPrefix(name, logs)
		return logs != "" && ok && runnerLog.MatchString(rest)
	}}
}

// runnerLog is the name of a log the runner writes.
var runnerLog = regexp.MustCompile(`^[0-9]{8}-[0-9]{6}\.jsonl$`)

// Entries returns the workspace's files and folders, relative to it,
// folders with a slash at the end: the files git knows or would take, in a
// work tree of git, and otherwise those a walk finds, dependencies, caches
// and version control left out. Only the first call waits for the list; a
// list grown old is returned while a new one is made.
func (x *FileIndex) Entries(ctx context.Context) ([]string, error) {
	x.mu.Lock()
	defer x.mu.Unlock()
	if x.at.IsZero() {
		x.entries, x.took, x.err = listEntries(ctx, x.workspace, x.skip)
		x.at = time.Now()
		return x.entries, x.err
	}
	if time.Since(x.at) > max(fileIndexFresh, 10*x.took) && !x.refreshing {
		x.refreshing = true
		go func() {
			entries, took, err := listEntries(context.Background(), x.workspace, x.skip)
			x.mu.Lock()
			x.entries, x.took, x.err, x.at, x.refreshing = entries, took, err, time.Now(), false
			x.mu.Unlock()
		}()
	}
	return x.entries, x.err
}

// listEntries lists the files and folders of a workspace but those skip
// leaves out, and how long that took.
func listEntries(ctx context.Context, workspace string, skip func(string) bool) ([]string, time.Duration, error) {
	started := time.Now()
	ctx, cancel := context.WithTimeout(ctx, indexTime)
	defer cancel()
	files, err := gitFiles(ctx, workspace)
	if err != nil {
		files, err = walkFiles(ctx, workspace)
	}
	if skip != nil {
		files = slices.DeleteFunc(files, skip)
	}
	return withFolders(files), time.Since(started), err
}

// gitFiles lists the files of a work tree of git that are tracked or not
// ignored.
func gitFiles(ctx context.Context, workspace string) ([]string, error) {
	cmd := exec.CommandContext(ctx, "git", "-C", workspace, "ls-files", "-z", "--cached", "--others", "--exclude-standard")
	out, err := cmd.Output()
	if err != nil {
		return nil, err
	}
	var files []string
	for name := range bytes.SplitSeq(out, []byte{0}) {
		if len(name) == 0 || bytes.HasPrefix(name, []byte(".harness/")) {
			continue
		}
		files = append(files, string(name))
		if len(files) == maxIndexed {
			break
		}
	}
	return files, nil
}

// skippedFolders are left out of a walk: version control, the harness's
// own, dependencies and caches.
var skippedFolders = []string{
	".git", ".hg", ".svn", ".harness", "node_modules", ".venv", "venv", "__pycache__", ".mypy_cache",
	".pytest_cache", ".tox", ".gradle", ".next", ".nuxt", ".turbo", ".terraform", ".cache", ".Trash",
}

// walkFiles lists the files under workspace, for a folder git does not
// keep.
func walkFiles(ctx context.Context, workspace string) ([]string, error) {
	var files []string
	errFull := errors.New("full")
	err := filepath.WalkDir(workspace, func(name string, entry fs.DirEntry, err error) error {
		if ctx.Err() != nil {
			return ctx.Err()
		}
		if err != nil {
			if entry != nil && entry.IsDir() && name != workspace {
				return filepath.SkipDir
			}
			return nil
		}
		if entry.IsDir() {
			if name != workspace && slices.Contains(skippedFolders, entry.Name()) {
				return filepath.SkipDir
			}
			return nil
		}
		rel, err := filepath.Rel(workspace, name)
		if err != nil {
			return nil
		}
		files = append(files, filepath.ToSlash(rel))
		if len(files) == maxIndexed {
			return errFull
		}
		return nil
	})
	if errors.Is(err, errFull) || errors.Is(err, context.DeadlineExceeded) {
		err = nil // as many as there was room and time for
	}
	return files, err
}

// withFolders adds to files the folders they are in, each once, and sorts
// them.
func withFolders(files []string) []string {
	seen := make(map[string]bool)
	entries := slices.Clone(files)
	for _, file := range files {
		for dir := path.Dir(file); dir != "." && dir != "/" && !seen[dir]; dir = path.Dir(dir) {
			seen[dir] = true
			entries = append(entries, dir+"/")
		}
	}
	slices.Sort(entries)
	return slices.Compact(entries)
}
