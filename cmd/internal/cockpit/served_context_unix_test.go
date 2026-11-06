//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package cockpit_test

import (
	"fmt"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"testing"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
)

func TestStartServedContextMatchesActualGateway(t *testing.T) {
	t.Setenv(runconfig.ServedContextWindowEnvironment, "777777")
	endpoint := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
		t.Error("the fake runner must not POST to a model")
	}))
	defer endpoint.Close()
	otherEndpoint := httptest.NewServer(http.HandlerFunc(func(http.ResponseWriter, *http.Request) {
		t.Error("the fake runner must not POST to another model")
	}))
	defer otherEndpoint.Close()
	short, err := os.MkdirTemp("/tmp", "served-lease-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { os.RemoveAll(short) })
	socket := filepath.Join(short, "lease.sock")
	listener, err := net.Listen("unix", socket)
	if err != nil {
		t.Fatal(err)
	}
	leased := make(chan string, 32)
	leaseServer := &http.Server{Handler: http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		if r.URL.Path != "/internal/lease" || r.Header.Get("Authorization") != "Bearer fixture-key" {
			http.Error(w, "wrong lease", http.StatusUnauthorized)
			return
		}
		owner := r.Header.Get("X-Kou-Gateway-Owner")
		leased <- owner
		w.Header().Set("X-Kou-Gateway-Owner", owner)
		w.Header().Set("Content-Length", "0")
		w.WriteHeader(http.StatusOK)
		w.(http.Flusher).Flush()
		<-r.Context().Done()
	})}
	go leaseServer.Serve(listener)
	t.Cleanup(func() { leaseServer.Close() })
	for _, detached := range []bool{false, true} {
		for _, test := range []struct {
			name, sourceOwner, actualOwner string
			direct, overrideBase, legacy   bool
			noCurrentHint                  bool
			want                           string
		}{
			{name: "same generation", sourceOwner: "A", actualOwner: "A", want: "1000000"},
			{name: "publication A replaced by B", sourceOwner: "A", actualOwner: "B"},
			{name: "empty source owner", actualOwner: "A"},
			{name: "ownerless embedded publication", legacy: true},
			{name: "direct provider", sourceOwner: "A", actualOwner: "A", direct: true},
			{name: "final base override", sourceOwner: "A", actualOwner: "A", overrideBase: true},
			{name: "no current metadata", actualOwner: "A", noCurrentHint: true},
		} {
			t.Run(fmt.Sprintf("%s/detached=%t", test.name, detached), func(t *testing.T) {
				settings := filepath.Join(t.TempDir(), "settings.json")
				g := cockpit.Gateway{URL: endpoint.URL, APIKey: "fixture-key", Owner: test.actualOwner, LeaseSocket: socket}
				if test.legacy {
					g.LeaseSocket = ""
				}
				if err := cockpit.PublishGateway(cockpit.GatewayPath(settings), g); err != nil {
					t.Fatal(err)
				}
				connection := cockpit.Settings{API: cockpit.APIGateway, AutoCompactPercent: 62.5}
				if test.direct {
					connection.API, connection.BaseURL, connection.APIKey = cockpit.APIResponses, endpoint.URL+"/v1", "fixture-key"
				}
				if err := cockpit.SaveSettings(settings, connection); err != nil {
					t.Fatal(err)
				}
				o := cockpit.Options{Runner: cockpittest.Runner(t), SettingsFile: settings, Workspace: t.TempDir(), SessionDir: t.TempDir()}
				if err := os.WriteFile(filepath.Join(o.Workspace, ".env"), []byte(runconfig.ServedContextWindowEnvironment+"=888888\n"), 0o600); err != nil {
					t.Fatal(err)
				}
				r := cockpit.Request{
					SessionID: "capacity", MessageID: uuid.New().String(), Prompt: "print-context-capacity", Model: "served-only-alias",
					ServedContextOwner: test.sourceOwner,
					Env:                []string{runconfig.ContextWindowEnvironment + "=64000", runconfig.CacheTTLEnvironment + "=1h"},
				}
				if !test.noCurrentHint {
					// An earlier canvas entry cannot supersede current model metadata.
					r.Env = append(r.Env, runconfig.ServedContextWindowEnvironment+"=999999", runconfig.ServedContextWindowEnvironment+"=1000000")
				}
				if test.overrideBase {
					r.Env = append(r.Env, "KOU_CONVEYOR_LLM_BASE_URL="+otherEndpoint.URL+"/v1")
				}
				if detached {
					r.Detached = filepath.Join(t.TempDir(), "job")
				}
				job, err := cockpit.Start(t.Context(), o, r)
				if err != nil {
					t.Fatal(err)
				}
				tr := cockpit.NewTranscript()
				drain(t, job, tr)
				if err := job.Err(); err != nil {
					t.Fatal(err)
				}
				want := "served=" + test.want + " present=true context=64000 ttl=1h auto=62.5%"
				if got := tr.Entries[len(tr.Entries)-1].Text; got != want {
					t.Fatalf("actual launch received %q; want %q", got, want)
				}
				select {
				case owner := <-leased:
					if test.legacy || test.overrideBase || owner != test.actualOwner {
						t.Fatalf("leased %q instead of the actual selected generation %q", owner, test.actualOwner)
					}
				default:
					if !test.legacy && !test.overrideBase {
						t.Fatal("the selected gateway was not leased")
					}
				}
			})
		}
	}
}
