package cockpit

import (
	"context"
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"reflect"
	"slices"
	"strings"
	"testing"
)

// workspaceWith makes a workspace with the files named, and the folders
// they are in; names ending in a slash are empty folders.
func workspaceWith(t *testing.T, names ...string) string {
	t.Helper()
	dir := t.TempDir()
	for _, name := range names {
		path := filepath.Join(dir, filepath.FromSlash(name))
		if strings.HasSuffix(name, "/") {
			if err := os.MkdirAll(path, 0o755); err != nil {
				t.Fatal(err)
			}
			continue
		}
		if err := os.MkdirAll(filepath.Dir(path), 0o755); err != nil {
			t.Fatal(err)
		}
		if err := os.WriteFile(path, []byte("content of "+name+"\n"), 0o644); err != nil {
			t.Fatal(err)
		}
	}
	return dir
}

// labels are the labels of links, with their lines where they ask for some.
func labels(links []FileLink) []string {
	var out []string
	for _, link := range links {
		one := link.Label + " → " + link.Path
		if link.StartLine != 0 || link.EndLine != 0 {
			one += fmt.Sprintf(" %d-%d", link.StartLine, link.EndLine)
		}
		if link.Directory {
			one += " (folder)"
		}
		out = append(out, one)
	}
	return out
}

func TestFileLinksFindTheFilesATextNames(t *testing.T) {
	workspace := workspaceWith(t, "a.go", "b.go", "cmd/main.go", "my file.txt", "notes.", "app/(auth)/page.tsx", "[id].tsx", "файл.txt", "HOME.md", "1x")
	for _, test := range []struct {
		text string
		want []string
	}{
		{"look at $a.go and $cmd/", []string{"$a.go → a.go", "$cmd/ → cmd/ (folder)"}},
		{"$cmd/main.go", []string{"$cmd/main.go → cmd/main.go"}},
		// What ends a sentence is no part of a path.
		{"see $a.go. And ($b.go), \"$cmd/main.go\"; or $a.go!", []string{"$a.go → a.go", "$b.go → b.go", "$cmd/main.go → cmd/main.go"}},
		// Unless the path with it exists.
		{"read $notes.", []string{"$notes. → notes."}},
		// Lines.
		{"$a.go:10 $a.go:10-20. $b.go:3-3,", []string{"$a.go:10 → a.go 10-0", "$a.go:10-20 → a.go 10-20", "$b.go:3-3 → b.go 3-3"}},
		{"$a.go:20-10 $a.go:0 $cmd:5 $a.go:x", nil},
		{"fix $a.go: it fails", []string{"$a.go → a.go"}},
		// Quoted paths, with lines.
		{`open $"my file.txt" and $"my file.txt":3-4,`, []string{`$"my file.txt" → my file.txt`, `$"my file.txt":3-4 → my file.txt 3-4`}},
		{`$"my file.txt`, nil},
		// What names nothing that exists is text.
		{"echo $HOME $1 $(pwd) ${X} $PATH/bin costs $5.", nil},
		// Code and escapes are text too.
		{"`$a.go` and ``x $b.go`` and\n```sh\ncat $a.go\n```\n", nil},
		{"\\$a.go and x$a.go", nil},
		{"`unclosed $a.go", []string{"$a.go → a.go"}},
		{"~~~\n$a.go\n~~~\n$b.go", []string{"$b.go → b.go"}},
		// Where a reference starts.
		{"($a.go [$b.go {$cmd/main.go <$a.go>", []string{"$a.go → a.go", "$b.go → b.go", "$cmd/main.go → cmd/main.go"}},
		{"x,$b.go", []string{"$b.go → b.go"}},
		// Paths of all sorts.
		{"$app/(auth)/page.tsx $[id].tsx $файл.txt $HOME.md", []string{"$app/(auth)/page.tsx → app/(auth)/page.tsx", "$[id].tsx → [id].tsx", "$файл.txt → файл.txt", "$HOME.md → HOME.md"}},
		{"$. $.. $./ $~ $/ $", nil},
		{"$./a.go", []string{"$./a.go → ./a.go"}},
		// Each label once.
		{"$a.go $a.go", []string{"$a.go → a.go"}},
	} {
		if got := labels(FileLinks(test.text, workspace)); !reflect.DeepEqual(got, test.want) {
			t.Errorf("FileLinks(%q) = %q, want %q", test.text, got, test.want)
		}
	}
}

func TestFileLinksOutsideTheWorkspace(t *testing.T) {
	home := workspaceWith(t, "notes.md")
	t.Setenv("HOME", home)
	other := workspaceWith(t, "x.txt")
	text := "$~/notes.md $" + filepath.Join(other, "x.txt")
	want := []string{"$~/notes.md → ~/notes.md", "$" + filepath.Join(other, "x.txt") + " → " + filepath.Join(other, "x.txt")}
	if got := labels(FileLinks(text, t.TempDir())); !reflect.DeepEqual(got, want) {
		t.Fatalf("FileLinks = %q, want %q", got, want)
	}
}

func TestFileLinksAreBounded(t *testing.T) {
	var names, refs []string
	for i := range MaxFileLinks + 5 {
		names = append(names, fmt.Sprintf("f%02d", i))
		refs = append(refs, fmt.Sprintf("$f%02d", i))
	}
	links := FileLinks(strings.Join(refs, " "), workspaceWith(t, names...))
	if len(links) != MaxFileLinks || links[0].Label != "$f00" {
		t.Fatalf("%d links", len(links))
	}
	// A reference as long as a paragraph is no path.
	if links := FileLinks("$"+strings.Repeat("a", maxReference+1), t.TempDir()); links != nil {
		t.Fatalf("links = %v", links)
	}
}

func TestLinkLabelsQuotePathsWithSpaces(t *testing.T) {
	for name, want := range map[string]string{"cmd/main.go": "$cmd/main.go", "my file.txt": `$"my file.txt"`, "cmd/": "$cmd/"} {
		if got := LinkLabel(name); got != want {
			t.Errorf("LinkLabel(%q) = %q, want %q", name, got, want)
		}
	}
	workspace := workspaceWith(t, "my file.txt")
	if links := FileLinks("see "+LinkLabel("my file.txt"), workspace); len(links) != 1 || links[0].Path != "my file.txt" {
		t.Fatalf("links = %v", links)
	}
}

func TestLinkQueryFindsTheReferenceBeingTyped(t *testing.T) {
	type query struct {
		start  int
		text   string
		quoted bool
	}
	for _, test := range []struct {
		before string
		want   *query
	}{
		{"look at $cmd/ma", &query{8, "cmd/ma", false}},
		{"$", &query{0, "", false}},
		{"look at $", &query{8, "", false}},
		{"see ($cm", &query{5, "cm", false}},
		{"a\n$doc", &query{2, "doc", false}},
		{`open $"my fi`, &query{5, "my fi", true}},
		{`$"a $b`, &query{0, "a $b", true}},
		{`$$x`, &query{0, "$x", false}},
		{"x$y", nil},
		{"\\$y", nil},
		{"look at $a.go and", nil},
		{"$a.go:10", nil},
		{"$a.go:10-", nil},
		{`$"done" and`, nil},
		{"`$a", nil}, // right after a backtick: code
		{"`$a`", nil},
		{"```\n$a", nil},
	} {
		start, text, quoted, ok := LinkQuery(test.before)
		var got *query
		if ok {
			got = &query{start, text, quoted}
		}
		if !reflect.DeepEqual(got, test.want) {
			t.Errorf("LinkQuery(%q) = %+v, want %+v", test.before, got, test.want)
		}
	}
}

var indexed = []string{
	"README.md", "cmd/", "cmd/kou-conveyor-tui/", "cmd/kou-conveyor-tui/model.go", "cmd/main.go",
	"docs/", "docs/plugins.md", "go.mod", "harness/", "harness/contextbuilder/", "harness/contextbuilder/linked.go",
}

func paths(matches []FileMatch) []string {
	var out []string
	for _, match := range matches {
		out = append(out, match.Path)
	}
	return out
}

func TestMatchFilesRanksWhatAReferenceCompletesTo(t *testing.T) {
	for _, test := range []struct {
		query string
		limit int
		want  []string
	}{
		// Nothing typed: the top level, folders first.
		{"", 10, []string{"cmd/", "docs/", "harness/", "go.mod", "README.md"}},
		// A folder: what is in it.
		{"cmd/", 10, []string{"cmd/kou-conveyor-tui/", "cmd/main.go"}},
		// Paths that start with the query, then names, then paths that hold
		// it, then those that hold its letters.
		{"cmd/k", 10, []string{"cmd/kou-conveyor-tui/", "cmd/kou-conveyor-tui/model.go"}},
		{"mod", 10, []string{"cmd/kou-conveyor-tui/model.go", "go.mod"}},
		{"MAIN", 10, []string{"cmd/main.go"}},
		{"linked", 10, []string{"harness/contextbuilder/linked.go"}},
		{"ctxbld", 10, []string{"harness/contextbuilder/", "harness/contextbuilder/linked.go"}},
		{"zzz", 10, nil},
		{"", 2, []string{"cmd/", "docs/"}},
	} {
		if got := paths(MatchFiles(indexed, test.query, test.limit)); !reflect.DeepEqual(got, test.want) {
			t.Errorf("MatchFiles(%q) = %q, want %q", test.query, got, test.want)
		}
	}
	if matches := MatchFiles(indexed, "cmd/", 1); !matches[0].Directory {
		t.Fatalf("matches = %+v", matches)
	}
}

func TestFileIndexListsWhatGitKeeps(t *testing.T) {
	if _, err := exec.LookPath("git"); err != nil {
		t.Skip("no git")
	}
	workspace := workspaceWith(t, "a.go", "cmd/main.go", "build/out.bin", ".gitignore", "new.txt", ".harness/sessions/x.jsonl")
	if err := os.WriteFile(filepath.Join(workspace, ".gitignore"), []byte("build/\n"), 0o644); err != nil {
		t.Fatal(err)
	}
	git := func(args ...string) {
		cmd := exec.Command("git", append([]string{"-C", workspace}, args...)...)
		cmd.Env = append(os.Environ(), "GIT_AUTHOR_NAME=t", "GIT_AUTHOR_EMAIL=t@t", "GIT_COMMITTER_NAME=t", "GIT_COMMITTER_EMAIL=t@t")
		if out, err := cmd.CombinedOutput(); err != nil {
			t.Fatalf("git %v: %v\n%s", args, err, out)
		}
	}
	git("init", "-q")
	git("add", "a.go", "cmd/main.go", ".gitignore")
	entries, err := NewFileIndex(workspace, filepath.Join(workspace, ".harness", "sessions"), "").Entries(t.Context())
	if err != nil {
		t.Fatal(err)
	}
	// Tracked files and those git would take, not what it ignores or the
	// harness's own.
	want := []string{".gitignore", "a.go", "cmd/", "cmd/main.go", "new.txt"}
	if !reflect.DeepEqual(entries, want) {
		t.Fatalf("entries = %q, want %q", entries, want)
	}
}

func TestFileIndexWalksAFolderGitDoesNotKeep(t *testing.T) {
	workspace := workspaceWith(t, "a.go", "src/b.go", "node_modules/x/index.js", ".git/HEAD", ".github/ci.yml", "empty/",
		"sessions/s.session.jsonl", "logs/20260927-101500.jsonl", "logs/notes.txt")
	t.Setenv("GIT_CEILING_DIRECTORIES", filepath.Dir(workspace))
	index := NewFileIndex(workspace, filepath.Join(workspace, "sessions"), "")
	entries, err := index.Entries(context.Background())
	if err != nil {
		t.Fatal(err)
	}
	// Sessions and the runner's logs in the workspace are the harness's own.
	want := []string{".github/", ".github/ci.yml", "a.go", "logs/", "logs/notes.txt", "src/", "src/b.go"}
	if !reflect.DeepEqual(entries, want) {
		t.Fatalf("entries = %q, want %q", entries, want)
	}
	// The list is kept: a file made since shows once it is listed anew.
	if err := os.WriteFile(filepath.Join(workspace, "c.go"), nil, 0o644); err != nil {
		t.Fatal(err)
	}
	if entries, _ := index.Entries(context.Background()); slices.Contains(entries, "c.go") {
		t.Fatal("the list was made anew at once")
	}
}

func TestCompleteGoesOutsideTheWorkspaceFromSlashOrHome(t *testing.T) {
	home := workspaceWith(t, "notes.md", "Notebook/x", ".hidden", "other.txt")
	t.Setenv("HOME", home)
	index := NewFileIndex(workspaceWith(t, "a.go"), "", "")
	matches, err := index.Complete(t.Context(), "~/no", 10)
	if err != nil {
		t.Fatal(err)
	}
	if got := paths(matches); !reflect.DeepEqual(got, []string{"~/Notebook/", "~/notes.md"}) || !matches[0].Directory {
		t.Fatalf("~/no completes to %q", got)
	}
	if got := paths(completePath("~/", 10)); !reflect.DeepEqual(got, []string{"~/Notebook/", "~/notes.md", "~/other.txt"}) {
		t.Fatalf("~/ completes to %q", got)
	}
	if got := paths(completePath("~/.h", 10)); !reflect.DeepEqual(got, []string{"~/.hidden"}) {
		t.Fatalf("~/.h completes to %q", got)
	}
	if got := paths(completePath(home+"/Notebook/", 10)); !reflect.DeepEqual(got, []string{home + "/Notebook/x"}) {
		t.Fatalf("completes to %q", got)
	}
	if got, _ := index.Complete(t.Context(), "a", 10); !reflect.DeepEqual(paths(got), []string{"a.go"}) {
		t.Fatalf("a completes to %q", paths(got))
	}
}

func TestMatchFilesLeavesOutLettersStrewnAlongAPath(t *testing.T) {
	entries := []string{"README.md", "harness/tool/command/command_test.go", "harness/llm/responsesapi/providers_stream_test.go"}
	if got := paths(MatchFiles(entries, "readme", 10)); !reflect.DeepEqual(got, []string{"README.md"}) {
		t.Fatalf("readme matches %q", got)
	}
}
