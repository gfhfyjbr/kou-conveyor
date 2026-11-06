package cockpit_test

import (
	"fmt"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"runtime"
	"testing"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

func compactionAnswer(t *testing.T, o cockpit.Options, request cockpit.Request) string {
	t.Helper()
	o.Runner = cockpittest.Runner(t)
	if o.Workspace == "" {
		o.Workspace = t.TempDir()
	}
	o.SessionDir = t.TempDir()
	request.SessionID, request.MessageID, request.Prompt = "percent", uuid.New().String(), "print-compaction"
	job, err := cockpit.Start(t.Context(), o, request)
	if err != nil {
		t.Fatal(err)
	}
	tr := cockpit.NewTranscript()
	drain(t, job, tr)
	if err := job.Err(); err != nil {
		t.Fatal(err)
	}
	return tr.Entries[len(tr.Entries)-1].Text
}

func TestStartCompactionSettingsAttachedAndDetached(t *testing.T) {
	t.Setenv(runconfig.AutoCompactEnvironment, "off")
	for _, detached := range []bool{false, true} {
		t.Run(fmt.Sprint("detached=", detached), func(t *testing.T) {
			if detached && runtime.GOOS == "windows" {
				t.Skip("detached processes use Unix process groups")
			}
			settings := filepath.Join(t.TempDir(), "settings.json")
			o := cockpit.Options{SettingsFile: settings}
			r := cockpit.Request{}
			if detached {
				r.Detached = filepath.Join(t.TempDir(), "job")
			}
			if err := cockpit.SaveSettings(settings, cockpit.Settings{AutoCompactPercent: 62.5}); err != nil {
				t.Fatal(err)
			}
			if got := compactionAnswer(t, o, r); got != "auto_compact=62.5%" {
				t.Fatalf("saved percent: %q", got)
			}
			r.Env = []string{runconfig.AutoCompactEnvironment + "=150k"}
			if got := compactionAnswer(t, o, r); got != "auto_compact=150k" {
				t.Fatalf("per-run override: %q", got)
			}
			r.Env = nil
			if err := cockpit.SaveSettings(settings, cockpit.Settings{}); err != nil {
				t.Fatal(err)
			}
			if got := compactionAnswer(t, o, r); got != "auto_compact=off" {
				t.Fatalf("default/reset did not inherit environment: %q", got)
			}
		})
	}
}

func TestStartCompactionSettingsThroughGatewayAndModel(t *testing.T) {
	t.Setenv(runconfig.AutoCompactEnvironment, "off")
	settings := filepath.Join(t.TempDir(), "settings.json")
	gateway := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
		t.Error("the fake runner should make no model request")
	}))
	defer gateway.Close()
	if err := cockpit.PublishGateway(cockpit.GatewayPath(settings), cockpit.Gateway{URL: gateway.URL, APIKey: "test-only"}); err != nil {
		t.Fatal(err)
	}
	if err := cockpit.SaveSettings(settings, cockpit.Settings{API: cockpit.APIGateway, AutoCompactPercent: 75}); err != nil {
		t.Fatal(err)
	}
	for _, model := range []string{"claude-opus-5-5", "gpt-6-sol"} {
		if got := compactionAnswer(t, cockpit.Options{SettingsFile: settings}, cockpit.Request{Model: model}); got != "auto_compact=75%" {
			t.Fatalf("%s: %q", model, got)
		}
	}
}

func TestStartExplicitProviderBypassesUnrelatedSettings(t *testing.T) {
	t.Setenv(runconfig.AutoCompactEnvironment, "150k")
	for _, test := range []struct {
		name, data, want string
	}{
		{"unknown connection", `{"provider_type":"invalid","base_url":"not-a-url","api_key":42,"auto_compact_percent":62.5}`, "62.5%"},
		{"gateway off", `{"provider_type":"gateway","auto_compact_percent":75}`, "75%"},
		{"malformed json", `{broken`, "150k"},
		{"invalid percent", `{"auto_compact_percent":101}`, "150k"},
		{"invalid percent type", `{"auto_compact_percent":"oops"}`, "150k"},
		{"legacy", `{"provider_type":"invalid"}`, "150k"},
	} {
		t.Run(test.name, func(t *testing.T) {
			settings := filepath.Join(t.TempDir(), "settings.json")
			if err := os.WriteFile(settings, []byte(test.data), 0o600); err != nil {
				t.Fatal(err)
			}
			if got := compactionAnswer(t, cockpit.Options{SettingsFile: settings, Provider: "ollama"}, cockpit.Request{}); got != "auto_compact="+test.want {
				t.Fatalf("%s: %q, want %s", test.name, got, test.want)
			}
		})
	}
	for _, path := range []string{filepath.Join(t.TempDir(), "missing.json"), t.TempDir()} {
		if got := compactionAnswer(t, cockpit.Options{SettingsFile: path, Provider: "ollama"}, cockpit.Request{}); got != "auto_compact=150k" {
			t.Fatalf("unreadable optional settings %q: %q", path, got)
		}
	}
}
