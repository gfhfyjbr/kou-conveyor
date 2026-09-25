package cockpit

import (
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestGoRunBuildsAMatchingRunner(t *testing.T) {
	root := goRunSource(filepath.Join(os.TempDir(), "go-build123", "b001", "exe", "kou-conveyor-tui"))
	if root == "" {
		t.Fatal("a go run build was not recognized")
	}
	if _, err := os.Stat(filepath.Join(root, "go.mod")); err != nil {
		t.Fatalf("root %s: %v", root, err)
	}
	for _, exe := range []string{"/usr/local/bin/kou-conveyor-tui", filepath.Join(root, "bin", "kou-conveyor-tui")} {
		if got := goRunSource(exe); got != "" {
			t.Errorf("%s counted as go run: %s", exe, got)
		}
	}
	if testing.Short() {
		t.Skip("builds the runner")
	}
	runner := filepath.Join(t.TempDir(), "kou-conveyor-runner")
	if err := buildRunner(root, runner); err != nil {
		t.Fatal(err)
	}
	out, err := exec.Command(runner, "-providers").Output()
	if err != nil || !strings.Contains(string(out), "anthropic") {
		t.Fatalf("providers %q, %v", out, err)
	}
	if err := checkRunner(runner, "anthropic"); err != nil {
		t.Fatal(err)
	}
}
