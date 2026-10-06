package fileops

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func write(t *testing.T, name, content string) string {
	t.Helper()
	if err := os.MkdirAll(filepath.Dir(name), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(name, []byte(content), 0o644); err != nil {
		t.Fatal(err)
	}
	return name
}

func read(t *testing.T, name string) string {
	t.Helper()
	data, err := os.ReadFile(name)
	if err != nil {
		t.Fatal(err)
	}
	return string(data)
}

func TestReadNumbersLines(t *testing.T) {
	name := write(t, filepath.Join(t.TempDir(), "a.txt"), "one\ntwo\nthree\n")
	result, err := Read(name, ReadOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if result.Text != "     1\tone\n     2\ttwo\n     3\tthree" || result.Lines != 3 || result.From != 1 || result.To != 3 || result.Truncated {
		t.Fatalf("result = %#v", result)
	}
}

func TestReadOffsetAndLimit(t *testing.T) {
	name := write(t, filepath.Join(t.TempDir(), "a.txt"), "1\n2\n3\n4\n5")
	result, err := Read(name, ReadOptions{Offset: 2, Limit: 2})
	if err != nil {
		t.Fatal(err)
	}
	if result.Text != "     2\t2\n     3\t3\n[Lines 2-3 of 5 shown; read on with offset=4.]" || !result.Truncated {
		t.Fatalf("result = %#v", result)
	}
	if _, err := Read(name, ReadOptions{Offset: 9}); err == nil || !strings.Contains(err.Error(), "past the end") {
		t.Fatalf("err = %v", err)
	}
	result, err = Read(name, ReadOptions{Offset: 4})
	if err != nil || result.Text != "     4\t4\n     5\t5\n[Lines 4-5 of 5 shown.]" {
		t.Fatalf("result = %#v, err = %v", result, err)
	}
}

func TestReadCutsLongLinesAndBoundsText(t *testing.T) {
	name := write(t, filepath.Join(t.TempDir(), "a.txt"), strings.Repeat("x", 30)+"\nshort\n")
	result, err := Read(name, ReadOptions{MaxLineBytes: 10})
	if err != nil {
		t.Fatal(err)
	}
	if !strings.Contains(result.Text, "xxxxxxxxxx … [line cut: 20 more bytes]") || !strings.Contains(result.Text, "short") {
		t.Fatalf("text = %q", result.Text)
	}
	result, err = Read(name, ReadOptions{MaxBytes: 10})
	if err != nil {
		t.Fatal(err)
	}
	if result.To != 1 || !result.Truncated || !strings.HasSuffix(result.Text, "\n[Lines 1-1 of 2 shown, about 10 bytes, the most one read shows; read on with offset=2, or find what you need with rg -n.]") {
		t.Fatalf("result = %#v", result)
	}
	// By default a read shows about 40,000 bytes.
	name = write(t, filepath.Join(t.TempDir(), "large.txt"), strings.Repeat(strings.Repeat("y", 99)+"\n", 1000))
	if result, err = Read(name, ReadOptions{}); err != nil || result.To >= 1000 || len(result.Text) > DefaultMaxReadBytes+1000 || !strings.Contains(result.Text, "rg -n") {
		t.Fatalf("result to %d of %d bytes, %v", result.To, len(result.Text), err)
	}
}

func TestReadRefusesWhatItCannotShow(t *testing.T) {
	directory := t.TempDir()
	if _, err := Read(filepath.Join(directory, "missing"), ReadOptions{}); err == nil || !strings.Contains(err.Error(), "does not exist") {
		t.Fatalf("err = %v", err)
	}
	if _, err := Read(directory, ReadOptions{}); err == nil || !strings.Contains(err.Error(), "is a directory") {
		t.Fatalf("err = %v", err)
	}
	binary := write(t, filepath.Join(directory, "a.png"), "\x89PNG\x00\x00")
	if _, err := Read(binary, ReadOptions{}); err == nil || !strings.Contains(err.Error(), "binary") || !strings.Contains(err.Error(), "ViewImage") {
		t.Fatalf("err = %v", err)
	}
	empty := write(t, filepath.Join(directory, "empty"), "")
	if result, err := Read(empty, ReadOptions{}); err != nil || result.Text != "(empty file)" {
		t.Fatalf("result = %#v, err = %v", result, err)
	}
}

func TestEditReplacesTheOneOccurrence(t *testing.T) {
	name := write(t, filepath.Join(t.TempDir(), "a.go"), "package a\n\nfunc A() int {\n\treturn 1\n}\n")
	result, err := Edit(name, "\treturn 1\n", "\t// two\n\treturn 2\n", EditOptions{})
	if err != nil {
		t.Fatal(err)
	}
	if got := read(t, name); got != "package a\n\nfunc A() int {\n\t// two\n\treturn 2\n}\n" {
		t.Fatalf("file = %q", got)
	}
	if result.Replacements != 1 || result.Line != 4 || result.Lines != 6 || !strings.Contains(result.Snippet, "     4\t\t// two") || !strings.HasPrefix(result.Snippet, "     1\tpackage a") {
		t.Fatalf("result = %#v", result)
	}
}

func TestEditRefusesAmbiguityUnlessReplaceAll(t *testing.T) {
	name := write(t, filepath.Join(t.TempDir(), "a.txt"), "x = 1\ny = 1\n")
	if _, err := Edit(name, "1", "2", EditOptions{}); err == nil || !strings.Contains(err.Error(), "appears 2 times") {
		t.Fatalf("err = %v", err)
	}
	result, err := Edit(name, "1", "2", EditOptions{ReplaceAll: true})
	if err != nil || result.Replacements != 2 || read(t, name) != "x = 2\ny = 2\n" {
		t.Fatalf("result = %#v, err = %v, file = %q", result, err, read(t, name))
	}
}

func TestEditExplainsWhatWasNotFound(t *testing.T) {
	name := write(t, filepath.Join(t.TempDir(), "a.txt"), "if x {\n    return\n}\n")
	_, err := Edit(name, "if x {\n\treturn\n}", "y", EditOptions{})
	if err == nil || !strings.Contains(err.Error(), "whitespace is ignored") {
		t.Fatalf("err = %v", err)
	}
	_, err = Edit(name, "if x {\n    return 5\n}", "y", EditOptions{})
	if err == nil || !strings.Contains(err.Error(), "first line is there") {
		t.Fatalf("err = %v", err)
	}
	_, err = Edit(name, "nothing", "y", EditOptions{})
	if err == nil || !strings.Contains(err.Error(), "was not found") {
		t.Fatalf("err = %v", err)
	}
	if _, err := Edit(name, "", "y", EditOptions{}); err == nil || !strings.Contains(err.Error(), "must not be empty") {
		t.Fatalf("err = %v", err)
	}
	if _, err := Edit(name, "x", "x", EditOptions{}); err == nil || !strings.Contains(err.Error(), "the same") {
		t.Fatalf("err = %v", err)
	}
}

func TestEditKeepsWindowsLineEndingsAndMode(t *testing.T) {
	name := write(t, filepath.Join(t.TempDir(), "a.bat"), "echo one\r\necho two\r\n")
	os.Chmod(name, 0o755)
	if _, err := Edit(name, "echo one\necho two\n", "echo three\n", EditOptions{}); err != nil {
		t.Fatal(err)
	}
	if got := read(t, name); got != "echo three\r\n" {
		t.Fatalf("file = %q", got)
	}
	if info, _ := os.Stat(name); info.Mode().Perm() != 0o755 {
		t.Fatalf("mode = %v", info.Mode())
	}
}

func TestWriteCreatesAndReplaces(t *testing.T) {
	name := filepath.Join(t.TempDir(), "deep", "dir", "a.txt")
	result, err := Write(name, "one\ntwo\n")
	if err != nil || !result.Created || result.Lines != 2 || result.Bytes != 8 || read(t, name) != "one\ntwo\n" {
		t.Fatalf("result = %#v, err = %v", result, err)
	}
	result, err = Write(name, "three")
	if err != nil || result.Created || result.PreviousLines != 2 || result.Lines != 1 || read(t, name) != "three" {
		t.Fatalf("result = %#v, err = %v", result, err)
	}
	if _, err := Write(filepath.Dir(name), "x"); err == nil || !strings.Contains(err.Error(), "is a directory") {
		t.Fatalf("err = %v", err)
	}
}

const samplePatch = `*** Begin Patch
*** Add File: added.txt
+first
+second
*** Update File: main.go
@@ func main() {
 	fmt.Println("one")
-	fmt.Println("two")
+	fmt.Println("2")
+	fmt.Println("2.5")
 	fmt.Println("three")
*** Delete File: gone.txt
*** End Patch
`

func TestApplyPatchAddsUpdatesAndDeletes(t *testing.T) {
	root := t.TempDir()
	write(t, filepath.Join(root, "main.go"), "package main\n\nfunc main() {\n\tfmt.Println(\"one\")\n\tfmt.Println(\"two\")\n\tfmt.Println(\"three\")\n}\n")
	write(t, filepath.Join(root, "gone.txt"), "bye\n")
	result, err := ApplyPatch(samplePatch, root)
	if err != nil {
		t.Fatal(err)
	}
	if got := read(t, filepath.Join(root, "main.go")); got != "package main\n\nfunc main() {\n\tfmt.Println(\"one\")\n\tfmt.Println(\"2\")\n\tfmt.Println(\"2.5\")\n\tfmt.Println(\"three\")\n}\n" {
		t.Fatalf("main.go = %q", got)
	}
	if got := read(t, filepath.Join(root, "added.txt")); got != "first\nsecond\n" {
		t.Fatalf("added.txt = %q", got)
	}
	if _, err := os.Stat(filepath.Join(root, "gone.txt")); !os.IsNotExist(err) {
		t.Fatalf("gone.txt: %v", err)
	}
	if result.Summary() != "Applied the patch:\nA added.txt\nM main.go\nD gone.txt" {
		t.Fatalf("summary = %q", result.Summary())
	}
}

func TestApplyPatchMatchesLooselyAndAtTheEnd(t *testing.T) {
	root := t.TempDir()
	name := write(t, filepath.Join(root, "a.py"), "def a():\n    return 1\n\n\ndef b():\n    return 2\n")
	patch := "*** Begin Patch\n*** Update File: a.py\n@@ def b():\n-    return 2   \n+    return 3\n*** End of File\n*** End Patch"
	if _, err := ApplyPatch(patch, root); err != nil {
		t.Fatal(err)
	}
	if got := read(t, name); got != "def a():\n    return 1\n\n\ndef b():\n    return 3\n" {
		t.Fatalf("a.py = %q", got)
	}
}

func TestApplyPatchManySectionsAndMove(t *testing.T) {
	root := t.TempDir()
	write(t, filepath.Join(root, "old.txt"), "a\nb\nc\nd\ne\nf\n")
	patch := "*** Begin Patch\n*** Update File: old.txt\n*** Move to: sub/new.txt\n@@\n a\n-b\n+B\n c\n@@\n e\n-f\n+F\n*** End Patch\n"
	result, err := ApplyPatch(patch, root)
	if err != nil {
		t.Fatal(err)
	}
	if got := read(t, filepath.Join(root, "sub", "new.txt")); got != "a\nB\nc\nd\ne\nF\n" {
		t.Fatalf("new.txt = %q", got)
	}
	if _, err := os.Stat(filepath.Join(root, "old.txt")); !os.IsNotExist(err) {
		t.Fatalf("old.txt: %v", err)
	}
	if len(result.Moved) != 1 || result.Moved[0] != "old.txt -> sub/new.txt" {
		t.Fatalf("result = %#v", result)
	}
}

func TestApplyPatchFailsWholeAndSaysWhy(t *testing.T) {
	root := t.TempDir()
	write(t, filepath.Join(root, "a.txt"), "one\ntwo\n")
	patch := "*** Begin Patch\n*** Add File: new.txt\n+x\n*** Update File: a.txt\n@@\n-three\n+3\n*** End Patch\n"
	_, err := ApplyPatch(patch, root)
	if err == nil || !strings.Contains(err.Error(), "a.txt: section 1") || !strings.Contains(err.Error(), "three") {
		t.Fatalf("err = %v", err)
	}
	if _, statErr := os.Stat(filepath.Join(root, "new.txt")); !os.IsNotExist(statErr) {
		t.Fatalf("new.txt was written before the patch failed: %v", statErr)
	}
	for _, bad := range []string{
		"no markers",
		"*** Begin Patch\n*** End Patch",
		"*** Begin Patch\n*** Update File: a.txt\n*** End Patch",
		"*** Begin Patch\n*** Add File: a.txt\n+x\n*** End Patch",
		"*** Begin Patch\n*** Delete File: missing\n*** End Patch",
		"*** Begin Patch\nbogus\n*** End Patch",
	} {
		if _, err := ApplyPatch(bad, root); err == nil {
			t.Fatalf("patch %q was accepted", bad)
		}
	}
}

func TestApplyPatchAcceptsTextAroundTheMarkers(t *testing.T) {
	root := t.TempDir()
	write(t, filepath.Join(root, "a.txt"), "one\n")
	patch := "Here is the patch:\n```\n*** Begin Patch\n*** Update File: a.txt\n@@\n-one\n+uno\n*** End Patch\n```\n"
	if _, err := ApplyPatch(patch, root); err != nil {
		t.Fatal(err)
	}
	if got := read(t, filepath.Join(root, "a.txt")); got != "uno\n" {
		t.Fatalf("a.txt = %q", got)
	}
}

func TestApplyPatchHeaderNarrowsTheSearch(t *testing.T) {
	root := t.TempDir()
	write(t, filepath.Join(root, "a.txt"), "class A:\n    x = 1\nclass B:\n    x = 1\n")
	patch := "*** Begin Patch\n*** Update File: a.txt\n@@ class B:\n-    x = 1\n+    x = 2\n*** End Patch\n"
	if _, err := ApplyPatch(patch, root); err != nil {
		t.Fatal(err)
	}
	if got := read(t, filepath.Join(root, "a.txt")); got != "class A:\n    x = 1\nclass B:\n    x = 2\n" {
		t.Fatalf("a.txt = %q", got)
	}
}

func TestWritesGoThroughSymlinks(t *testing.T) {
	directory := t.TempDir()
	target := write(t, filepath.Join(directory, "real.txt"), "one\n")
	link := filepath.Join(directory, "link.txt")
	if err := os.Symlink("real.txt", link); err != nil {
		t.Fatal(err)
	}
	if _, err := Edit(link, "one", "two", EditOptions{}); err != nil {
		t.Fatal(err)
	}
	if _, err := Write(link, "three\n"); err != nil {
		t.Fatal(err)
	}
	if _, err := ApplyPatch("*** Begin Patch\n*** Update File: link.txt\n@@\n-three\n+four\n*** End Patch", directory); err != nil {
		t.Fatal(err)
	}
	if info, err := os.Lstat(link); err != nil || info.Mode()&os.ModeSymlink == 0 {
		t.Fatalf("link.txt is no link any more: %v", err)
	}
	if got := read(t, target); got != "four\n" {
		t.Fatalf("real.txt = %q", got)
	}
}

func TestApplyPatchSeesWhatTheSectionsBeforeDid(t *testing.T) {
	root := t.TempDir()
	write(t, filepath.Join(root, "a.txt"), "one\ntwo\nthree\n")
	write(t, filepath.Join(root, "c.txt"), "old\n")
	patch := "*** Begin Patch\n" +
		"*** Update File: a.txt\n@@\n-one\n+1\n" +
		"*** Update File: a.txt\n@@\n-three\n+3\n" +
		"*** Add File: b.txt\n+x\n" +
		"*** Update File: b.txt\n@@\n-x\n+y\n" +
		"*** Delete File: c.txt\n" +
		"*** Add File: c.txt\n+new\n" +
		"*** End Patch"
	result, err := ApplyPatch(patch, root)
	if err != nil {
		t.Fatal(err)
	}
	for name, want := range map[string]string{"a.txt": "1\ntwo\n3\n", "b.txt": "y\n", "c.txt": "new\n"} {
		if got := read(t, filepath.Join(root, name)); got != want {
			t.Fatalf("%s = %q, want %q", name, got, want)
		}
	}
	if result.Summary() != "Applied the patch:\nA b.txt\nA c.txt\nM a.txt\nD c.txt" {
		t.Fatalf("summary = %q", result.Summary())
	}
	// A section that fails still fails the whole patch.
	patch = "*** Begin Patch\n*** Update File: a.txt\n@@\n-1\n+one\n*** Update File: a.txt\n@@\n-1\n+uno\n*** End Patch"
	if _, err := ApplyPatch(patch, root); err == nil || !strings.Contains(err.Error(), "a.txt: section 1") {
		t.Fatalf("err = %v", err)
	}
	if got := read(t, filepath.Join(root, "a.txt")); got != "1\ntwo\n3\n" {
		t.Fatalf("a.txt = %q", got)
	}
}

func TestApplyPatchKeepsWindowsLineEndings(t *testing.T) {
	root := t.TempDir()
	name := write(t, filepath.Join(root, "a.bat"), "echo one\r\necho two\r\n")
	patch := "*** Begin Patch\n*** Update File: a.bat\n@@\n echo one\n-echo two\n+echo 2\n+echo 3\n*** End Patch"
	if _, err := ApplyPatch(patch, root); err != nil {
		t.Fatal(err)
	}
	if got := read(t, name); got != "echo one\r\necho 2\r\necho 3\r\n" {
		t.Fatalf("a.bat = %q", got)
	}
}

func TestApplyPatchForgivesBlankLinesAroundSections(t *testing.T) {
	root := t.TempDir()
	name := write(t, filepath.Join(root, "a.txt"), "one\ntwo\nthree\n")
	// A blank line before the first @@, and a blank context line at the end
	// that the file does not have.
	patch := "*** Begin Patch\n*** Update File: a.txt\n\n@@\n one\n-two\n+2\n three\n\n*** End Patch"
	if _, err := ApplyPatch(patch, root); err != nil {
		t.Fatal(err)
	}
	if got := read(t, name); got != "one\n2\nthree\n" {
		t.Fatalf("a.txt = %q", got)
	}
}
