package cockpit

import (
	"encoding/json/v2"
	"math"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

func TestCompactionSettingsRoundTripAndLegacy(t *testing.T) {
	path := filepath.Join(t.TempDir(), "settings.json")
	for _, api := range []API{APIEnvironment, APIResponses, APIMessages, APIGateway} {
		want := Settings{API: api, AutoCompactPercent: 62.5}
		if err := SaveSettings(path, want); err != nil {
			t.Fatal(err)
		}
		got, err := LoadSettings(path)
		if err != nil || got != want {
			t.Fatalf("%s: round trip = %+v, %v; want %+v", api, got, err, want)
		}
	}
	for _, legacy := range []string{`{}`, `{"provider_type":"responses","model":"old"}`, `{"provider_type":"gateway"}`} {
		if err := os.WriteFile(path, []byte(legacy), 0o600); err != nil {
			t.Fatal(err)
		}
		got, err := LoadSettings(path)
		if err != nil || got.AutoCompactPercent != 0 {
			t.Fatalf("legacy %s: %+v, %v", legacy, got, err)
		}
	}
}

func TestCompactionSettingsValidation(t *testing.T) {
	for _, percent := range []float64{-1, 100.01, math.NaN(), math.Inf(1), math.Inf(-1)} {
		s := Settings{AutoCompactPercent: percent}
		if err := s.Validate(); err == nil || !strings.Contains(err.Error(), "compaction percent") {
			t.Errorf("percent %v: %v", percent, err)
		}
		if err := SaveSettings(filepath.Join(t.TempDir(), "settings.json"), s); err == nil {
			t.Errorf("saved invalid percent %v", percent)
		}
	}
	for _, percent := range []float64{0, math.SmallestNonzeroFloat64, 0.1, 62.5, 100} {
		if err := (Settings{AutoCompactPercent: percent}).Validate(); err != nil {
			t.Errorf("valid percent %v: %v", percent, err)
		}
	}
	path := filepath.Join(t.TempDir(), "settings.json")
	for _, value := range []string{"-1", "101", `"NaN"`, `"80"`, "1e999"} {
		if err := os.WriteFile(path, []byte(`{"auto_compact_percent":`+value+`}`), 0o600); err != nil {
			t.Fatal(err)
		}
		if _, err := LoadSettings(path); err == nil {
			t.Errorf("loaded invalid percent %s", value)
		}
	}
}

func TestCompactionSettingsUpdatePreservesAndResets(t *testing.T) {
	saved := Settings{API: APIMessages, Model: "old", APIKey: "secret", AutoCompactPercent: 62.5}
	for _, api := range []API{APIEnvironment, APIResponses, APIMessages, APIGateway} {
		u := SettingsUpdate{API: api, APIKey: new("new-secret")}
		next, err := saved.Update(u)
		if err != nil || next.AutoCompactPercent != saved.AutoCompactPercent {
			t.Fatalf("%s omitted: %+v, %v", api, next, err)
		}
		if (api == APIEnvironment || api == APIGateway) && (next.APIKey != "" || next.BaseURL != "" || next.Model != "" || next.Protocol != "") {
			t.Fatalf("%s kept old connection fields: %+v", api, next)
		}
		u.AutoCompactPercent = new(0.0)
		next, err = saved.Update(u)
		if err != nil || next.AutoCompactPercent != 0 {
			t.Fatalf("%s reset: %+v, %v", api, next, err)
		}
		u.AutoCompactPercent = new(80.0)
		next, err = saved.Update(u)
		if err != nil || next.AutoCompactPercent != 80 {
			t.Fatalf("%s changed: %+v, %v", api, next, err)
		}
		u.AutoCompactPercent = new(-1.0)
		if _, err := saved.Update(u); err == nil {
			t.Errorf("%s update accepted a negative percent", api)
		}
	}
	var legacy SettingsUpdate
	if err := json.Unmarshal([]byte(`{"provider_type":"messages","model":"next"}`), &legacy); err != nil {
		t.Fatal(err)
	}
	next, err := saved.Update(legacy)
	if err != nil || next.AutoCompactPercent != saved.AutoCompactPercent || next.APIKey != saved.APIKey {
		t.Fatalf("legacy partial update: %+v, %v", next, err)
	}
}

func TestCompactionSettingsEnvironmentPrecedence(t *testing.T) {
	for _, inherited := range []string{"", "off", "150k", "90%"} {
		env := []string{"PATH=/bin", runconfig.AutoCompactEnvironment + "=" + inherited}
		before := slices.Clone(env)
		for _, api := range []API{APIEnvironment, APIResponses, APIMessages, APIGateway} {
			s := Settings{API: api}
			if got := s.Environment(env); envValue(got, runconfig.AutoCompactEnvironment) != inherited {
				t.Fatalf("%s default replaced inherited %q: %v", api, inherited, got)
			}
			s.AutoCompactPercent = 62.5
			got := s.Environment(env)
			if envValue(got, runconfig.AutoCompactEnvironment) != "62.5%" || !slices.Equal(env, before) {
				t.Fatalf("%s configured with inherited %q: %v; original %v", api, inherited, got, env)
			}
		}
	}
	workspace := t.TempDir()
	if err := os.WriteFile(filepath.Join(workspace, ".env"), []byte(runconfig.AutoCompactEnvironment+"=90%\n"), 0o600); err != nil {
		t.Fatal(err)
	}
	for _, percent := range []float64{0, 62.5} {
		env := (Settings{AutoCompactPercent: percent}).Environment(nil)
		t.Setenv(runconfig.AutoCompactEnvironment, envValue(env, runconfig.AutoCompactEnvironment))
		if percent == 0 {
			if err := os.Unsetenv(runconfig.AutoCompactEnvironment); err != nil {
				t.Fatal(err)
			}
		}
		want := "62.5%"
		if percent == 0 {
			want = "90%"
		}
		if got := WorkspaceEnv(workspace)(runconfig.AutoCompactEnvironment); got != want {
			t.Fatalf("saved percent %v vs workspace .env = %q; want %q", percent, got, want)
		}
	}
	s := Settings{API: APIGateway, AutoCompactPercent: 75}
	for _, model := range []string{"claude-opus-5-5", "gpt-6-sol"} {
		resolved := s.ForModel(model).Through(Gateway{URL: "http://127.0.0.1:8318", APIKey: "private"})
		if envValue(resolved.Environment(nil), runconfig.AutoCompactEnvironment) != "75%" {
			t.Fatalf("gateway/model projection lost percent: %+v", resolved)
		}
	}
}

func envValue(env []string, name string) string {
	for _, entry := range env {
		if value, ok := strings.CutPrefix(entry, name+"="); ok {
			return value
		}
	}
	return ""
}
