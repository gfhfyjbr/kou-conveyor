package canvas

import (
	"context"
	"encoding/json/v2"
	"net/http"
	"net/http/httptest"
	"reflect"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func TestAgentModelsUseTheWorkspacesCatalog(t *testing.T) {
	te := newEngine(t)
	id, c := te.newCanvas(t, "Models")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"kind":"agent","title":"Observer"}},
		{"op":"node.add","node":{"preset":"webhook","title":"Source"}}
	]`)
	node, source := res.IDs["0"], res.IDs["1"]
	rev := c.view(node, ScopeObserve, false).Canvas.Rev
	listed := cockpit.Catalog{Source: "gateway", Default: "sol/claude-alias", Models: []cockpit.ModelInfo{
		{ID: "sol/claude-alias", Name: "Claude upstream display name", Provider: "anthropic", ProviderName: "Anthropic",
			API: cockpit.APIMessages, Context: 200000, Cooling: true, Via: []string{"sol"}},
		{ID: "gpt-image-2", Media: true}, {ID: ""}, {ID: "two words"}, {ID: strings.Repeat("x", 257)},
	}}
	requests := make(chan bool, 4)
	te.host.models = func(ctx context.Context, ws Workspace, fresh bool) cockpit.Catalog {
		if ws != te.host.ws {
			t.Errorf("catalog workspace = %+v, want %+v", ws, te.host.ws)
		}
		if deadline, ok := ctx.Deadline(); !ok || time.Until(deadline) > 25*time.Second {
			t.Error("the model request has no bounded deadline")
		}
		requests <- fresh
		return listed
	}
	for _, test := range []struct {
		token string
		want  int
	}{
		{"", http.StatusUnauthorized},
		{"kc1.forged", http.StatusUnauthorized},
		{te.token(t, c, node, ScopeNone), http.StatusForbidden},
		{te.token(t, c, source, scopeSource), http.StatusForbidden},
	} {
		if status, body := te.call(t, "GET", "/api/canvas/models", "", test.token); status != test.want {
			t.Fatalf("model authorization: %d %s, want %d", status, body, test.want)
		}
	}
	if len(requests) != 0 {
		t.Fatal("an unauthorized call reached the catalog")
	}
	for _, query := range []string{"?fresh=1&ws=some-other-workspace", "", "?fresh=false"} {
		var got agentModelCatalog
		te.must(t, http.StatusOK, "GET", "/api/canvas/models"+query, "", te.token(t, c, node, ScopeObserve), &got)
		if fresh := <-requests; fresh != strings.Contains(query, "fresh=1") {
			t.Fatalf("%s: fresh = %v", query, fresh)
		}
		if got.Source != "gateway" || got.Default != listed.Default || got.Error != "" || len(got.Models) != 1 || got.Omitted != 3 {
			t.Fatalf("catalog = %+v", got)
		}
		if !reflect.DeepEqual(got.Models[0], listed.Models[0]) {
			t.Fatalf("alias/metadata changed: %+v, want %+v", got.Models[0], listed.Models[0])
		}
	}
	if len(listed.Models) != 5 || listed.Models[1].ID != "gpt-image-2" {
		t.Fatal("the agent listing mutated the shared picker catalog")
	}
	if c.view(node, ScopeObserve, false).Canvas.Rev != rev || len(te.host.promptsOf(c.node(node).Runtime.Session)) != 0 {
		t.Fatal("listing models changed the canvas or prompted its agent")
	}
}

func TestAgentModelsEmptyAndUnavailable(t *testing.T) {
	for _, test := range []struct {
		name   string
		listed cockpit.Catalog
	}{
		{"no catalog", cockpit.Catalog{}},
		{"empty", cockpit.Catalog{Source: "gateway"}},
		{"unavailable", cockpit.Catalog{Error: "cannot reach https://private.example/v1?key=sk-private"}},
	} {
		t.Run(test.name, func(t *testing.T) {
			te := newEngine(t)
			id, c := te.newCanvas(t, "Models")
			res := te.ops(t, id, `[{"op":"node.add","node":{"kind":"agent"}}]`)
			te.host.models = func(context.Context, Workspace, bool) cockpit.Catalog { return test.listed }
			status, body := te.call(t, "GET", "/api/canvas/models", "", te.token(t, c, res.IDs["0"], ScopeObserve))
			var got cockpit.Catalog
			if status != http.StatusOK || json.Unmarshal(body, &got) != nil || got.Models == nil || len(got.Models) != 0 {
				t.Fatalf("empty catalog = %d %s", status, body)
			}
			if (got.Error != "") != (test.listed.Error != "") {
				t.Fatalf("error = %q", got.Error)
			}
			if strings.Contains(string(body), "private") || strings.Contains(string(body), "https://") {
				t.Fatalf("private connection details in the result: %s", body)
			}
		})
	}
}

func TestAgentModelsCancellation(t *testing.T) {
	te := newEngine(t)
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	r := httptest.NewRequest("GET", "/api/canvas/models", nil).WithContext(ctx)
	w := httptest.NewRecorder()
	te.handleAgentModels(w, r, &agentCall{c: &Canvas{ws: te.host.ws}})
	if !strings.Contains(w.Body.String(), "cancelled") {
		t.Fatalf("cancelled catalog = %s", w.Body.String())
	}
}
