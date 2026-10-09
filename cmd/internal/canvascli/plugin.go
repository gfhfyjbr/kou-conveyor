package canvascli

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
)

// The canvas-agent plugin of the harness gives kou agents on a canvas the
// canvas's tools: its plugin.json is made of the tools here, by go
// generate, and a test checks that the two agree.

//go:generate sh -c "go run ../../kou-conveyor-canvas plugin-json > ../../../harness/plugin/builtin/canvas-agent/plugin.json"

// PluginManifest is the canvas-agent plugin's plugin.json.
func PluginManifest() ([]byte, error) {
	type tool struct {
		Name        string         `json:"name"`
		Description string         `json:"description"`
		Parameters  jsontext.Value `json:"parameters"`
		Run         []string       `json:"run"`
	}
	manifest := struct {
		Name        string `json:"name"`
		Description string `json:"description"`
		Tools       []tool `json:"tools"`
		Prompt      string `json:"prompt"`
		Skills      string `json:"skills"`
		Requires    struct {
			Env []string `json:"env"`
		} `json:"requires"`
	}{
		Name: "canvas-agent",
		Description: "The canvas's tools for kou agents on a canvas: see the board, create and wire nodes, send them work and read what they answer. " +
			"Only agents on a canvas have it. Generated from kou-canvas's tools: go generate ./cmd/internal/canvascli.",
		Prompt: "prompt.md",
		Skills: "skills",
	}
	manifest.Requires.Env = []string{envToken}
	for _, t := range tools {
		manifest.Tools = append(manifest.Tools, tool{
			Name: t.Name, Description: t.Description, Parameters: jsontext.Value(t.Schema),
			Run: []string{"kou-canvas", "tool", t.Name},
		})
	}
	data, err := json.Marshal(manifest, jsontext.WithIndent("  "))
	if err != nil {
		return nil, err
	}
	return append(data, '\n'), nil
}
