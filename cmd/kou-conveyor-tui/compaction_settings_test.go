package main

import (
	"io"
	"path/filepath"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

func TestHeadlessAppliesSavedCompactionPercent(t *testing.T) {
	t.Setenv(runconfig.AutoCompactEnvironment, "off")
	workspace := t.TempDir()
	path := filepath.Join(t.TempDir(), "settings.json")
	if err := cockpit.SaveSettings(path, cockpit.Settings{AutoCompactPercent: 62.5}); err != nil {
		t.Fatal(err)
	}
	o := options{prompt: "print-compaction", historyFile: filepath.Join(workspace, "history.json")}
	o.Workspace, o.SessionDir, o.Runner, o.SettingsFile = workspace, filepath.Join(workspace, "sessions"), cockpittest.Runner(t), path
	var out strings.Builder
	if err := runHeadless(t.Context(), o, &out, io.Discard); err != nil || out.String() != "auto_compact=62.5%\n" {
		t.Fatalf("headless output=%q, err=%v", out.String(), err)
	}
	if err := cockpit.SaveSettings(path, cockpit.Settings{}); err != nil {
		t.Fatal(err)
	}
	out.Reset()
	if err := runHeadless(t.Context(), o, &out, io.Discard); err != nil || out.String() != "auto_compact=off\n" {
		t.Fatalf("headless reset output=%q, err=%v", out.String(), err)
	}
}
