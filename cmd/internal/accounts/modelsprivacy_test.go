package accounts

import (
	"encoding/json/v2"
	"fmt"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"testing"

	"github.com/router-for-me/CLIProxyAPI/v8/sdk/cliproxy"
)

func TestModelAttributionOmitsPrivateClientFallbacks(t *testing.T) {
	const modelID = "sol/claude-alias-privacy-test"
	const privateID = "private-credential-file.json"
	const unresolvedID = "unresolved-private-registry-client"
	const endpointID = "openai-compatibility:sol:private-key-hash"
	reg := cliproxy.GlobalModelRegistry()
	for _, id := range []string{privateID, unresolvedID, endpointID} {
		reg.RegisterClient(id, "openai", []*cliproxy.ModelInfo{{ID: modelID, DisplayName: "Claude display name", OwnedBy: "anthropic"}})
		t.Cleanup(func() { reg.UnregisterClient(id) })
	}
	for _, test := range []struct {
		name   string
		status int
		body   string
		via    []string
	}{
		{"lookup failed", http.StatusServiceUnavailable, `{"error":"private management lookup failure"}`, []string{"sol"}},
		{"unresolved clients", http.StatusOK, `{"files":[]}`, []string{"sol"}},
		{"no human label", http.StatusOK, `{"files":[{"id":"private-credential-file.json","name":"private-credential-file.json","provider":"claude"}]}`, []string{"Claude", "sol"}},
		{"no key label", http.StatusOK, `{"files":[{"id":"private-credential-file.json","name":"private-credential-file.json","provider":"claude","account_type":"api_key","account":"sk-private-credential"}]}`, []string{"Claude", "sol"}},
		{"mixed resolved and unresolved", http.StatusOK, `{"files":[{"id":"private-credential-file.json","name":"private-credential-file.json","provider":"claude","email":"named@example.test"}]}`, []string{"Claude · named@example.test", "sol"}},
	} {
		t.Run(test.name, func(t *testing.T) {
			srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path != "/v0/management/auth-files" {
					t.Errorf("unexpected management request: %s", r.URL.Path)
				}
				w.WriteHeader(test.status)
				fmt.Fprint(w, test.body)
			}))
			defer srv.Close()
			h := &Host{client: newClient(srv.URL, "management-test"), clients: newClientWatch()}
			h.clients.add(privateID, unresolvedID, endpointID)
			models, err := h.Models(t.Context())
			if err != nil {
				t.Fatal(err)
			}
			var found *Model
			for i := range models {
				if models[i].ID == modelID {
					found = &models[i]
					break
				}
			}
			if found == nil || found.Name != "Claude display name" {
				t.Fatalf("the model selector or name was changed or removed: %+v", models)
			}
			got := slices.Clone(found.Via)
			slices.Sort(got)
			if !slices.Equal(got, test.via) {
				t.Fatalf("safe route labels = %v, want %v", got, test.via)
			}
			body, err := json.Marshal(found.Via)
			if err != nil {
				t.Fatal(err)
			}
			for _, private := range []string{privateID, unresolvedID, endpointID, "sk-private", "private-key-hash"} {
				if strings.Contains(string(body), private) {
					t.Errorf("model attribution contains %q: %s", private, body)
				}
			}
		})
	}
}
