//go:build darwin && cgo

package spawn_test

import (
	"context"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

func TestExecWithBlockedPreemption(t *testing.T) {
	if testing.Short() {
		t.Skip("builds a native signal-mask fixture")
	}
	ctx, cancel := context.WithTimeout(t.Context(), 45*time.Second)
	defer cancel()
	helper := filepath.Join(t.TempDir(), "exec")
	if out, err := spawn.CommandContext(ctx, "go", "build", "-o", helper, "./testdata/exec").CombinedOutput(); err != nil {
		t.Fatalf("build fixture: %v\n%s", err, out)
	}
	for range 3 {
		ctx, cancel := context.WithTimeout(t.Context(), 5*time.Second)
		out, err := spawn.CommandContext(ctx, helper).CombinedOutput()
		cancel()
		if err != nil || !strings.Contains(string(out), "executed\n") {
			t.Fatalf("Exec with blocked SIGURG: %v\n%s", err, out)
		}
	}
}
