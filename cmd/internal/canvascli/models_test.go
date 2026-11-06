package canvascli

import (
	"context"
	"encoding/json/v2"
	"fmt"
	"net/http"
	"net/http/httptest"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

const modelsFixture = `{"source":"gateway","default":"sol/claude-alias","models":[
	{"id":"sol/claude-alias","name":"Claude display name","provider":"anthropic","provider_name":"Anthropic",
	 "api":"messages","context":200000,"cooling":true,"via":["sol"]},
	{"id":"unknown-alias"}
]}`

func TestCanvasListModelsAndSpawnWithAnAlias(t *testing.T) {
	f := newFakeCanvas(t)
	tool, found := toolNamed("CanvasListModels")
	if !found || !tool.Refetch || !strings.Contains(tool.Description, "config.model") {
		t.Fatalf("model tool = %+v, found = %v", tool, found)
	}
	c := &client{base: f.server.URL, token: "kc1.test", http: f.server.Client()}
	for _, args := range []string{"", `{}`, `{"fresh":false}`, `{"fresh":true}`, `{"fresh":"true"}`} {
		res, err := tool.call(t.Context(), c, []byte(args))
		if err != nil {
			t.Fatal(err)
		}
		for _, want := range []string{"config.model", "id: sol/claude-alias", "name: Claude display name", "provider: Anthropic", "via: sol", "API: messages", "context: 200000 tokens", "cooling:", "id: unknown-alias", "unknown"} {
			if !strings.Contains(res.text, want) {
				t.Errorf("%s: missing %q in:\n%s", args, want, res.text)
			}
		}
		requests := f.take()
		if len(requests) != 1 || requests[0].method != "GET" || requests[0].path != "/api/canvas/models" {
			t.Fatalf("model requests = %+v", requests)
		}
		wantQuery := ""
		if strings.Contains(args, "true") {
			wantQuery = "fresh=1"
		}
		if requests[0].query != wantQuery {
			t.Fatalf("%s: query = %q, want %q", args, requests[0].query, wantQuery)
		}
		if string(res.raw) != modelsFixture {
			t.Fatalf("JSON catalog changed: %s", res.raw)
		}
	}
	spawn, _ := toolNamed("CanvasSpawn")
	if _, err := spawn.call(t.Context(), c, []byte(`{"config":{"model":"sol/claude-alias"}}`)); err != nil {
		t.Fatal(err)
	}
	node := f.take()[0].body["ops"].([]any)[0].(map[string]any)["node"].(map[string]any)
	if model := node["config"].(map[string]any)["model"]; model != "sol/claude-alias" {
		t.Fatalf("spawn model = %v", model)
	}
	if _, err := tool.call(t.Context(), c, []byte(`{"fresh":{}}`)); err == nil {
		t.Fatal("an object was accepted as fresh")
	}
}

func TestModelsCommand(t *testing.T) {
	f := newFakeCanvas(t)
	code, out, errOut := f.run(t, "", "models", "--fresh", "--json")
	var catalog cockpit.Catalog
	if code != 0 || json.Unmarshal([]byte(out), &catalog) != nil || catalog.Default != "sol/claude-alias" || len(catalog.Models) != 2 {
		t.Fatalf("models: %d %s %s", code, out, errOut)
	}
	if query := f.take()[0].query; query != "fresh=1" {
		t.Fatalf("models --fresh query = %q", query)
	}
	if code, out, errOut := f.run(t, `{}`, "tool", "CanvasListModels"); code != 0 || !strings.Contains(out, "id: sol/claude-alias") {
		t.Fatalf("tool: %d %s %s", code, out, errOut)
	}
	if code, _, _ := f.run(t, "", "models", "unexpected"); code == 0 {
		t.Fatal("models accepted a positional argument")
	}
}

func TestModelListingEmptyAndErrorCases(t *testing.T) {
	for _, test := range []struct {
		name, body, want string
		status           int
		isError          bool
	}{
		{"unavailable catalog", `{"models":[],"error":"check the connection"}`, "Catalog unavailable: check the connection", 200, false},
		{"no catalog", `{"models":[]}`, "does not provide a model catalog", 200, false},
		{"empty catalog", `{"source":"gateway","models":[]}`, "No agent models are listed", 200, false},
		{"unlisted default", `{"source":"endpoint","default":"old-model","models":[]}`, "not listed; availability unverified", 200, false},
		{"omitted invalid IDs", `{"source":"endpoint","models":[],"omitted":2}`, "Omitted 2 catalog entries with invalid model IDs; listed IDs are unchanged", 200, false},
		{"forbidden", `{"error":"this needs observe access"}`, "this needs observe access", 403, true},
		{"malformed", `{"models":`, "not what kou-canvas expects", 200, true},
	} {
		t.Run(test.name, func(t *testing.T) {
			srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				w.WriteHeader(test.status)
				fmt.Fprint(w, test.body)
			}))
			defer srv.Close()
			c := &client{base: srv.URL, token: "kc1.test", http: srv.Client()}
			res, err := c.listModels(context.Background(), listModelsArgs{})
			if test.isError {
				if err == nil || !strings.Contains(err.Error(), test.want) {
					t.Fatalf("error = %v, want %q", err, test.want)
				}
			} else if err != nil || !strings.Contains(res.text, test.want) {
				t.Fatalf("result = %q, %v; want %q", res.text, err, test.want)
			}
		})
	}
}

func TestMCPDiscoversModelListing(t *testing.T) {
	f := newFakeCanvas(t)
	input := `{"jsonrpc":"2.0","id":1,"method":"tools/list"}` + "\n" +
		`{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"CanvasListModels","arguments":{}}}` + "\n"
	code, out, errOut := f.run(t, input, "mcp")
	if code != 0 || !strings.Contains(out, `"name":"CanvasListModels"`) || !strings.Contains(out, `"fresh":{"type":"boolean","default":false`) || !strings.Contains(out, "id: sol/claude-alias") || !strings.Contains(out, `"isError":false`) {
		t.Fatalf("MCP models: %d %s %s", code, out, errOut)
	}
}
