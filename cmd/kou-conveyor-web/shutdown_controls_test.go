//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package main

import (
	"context"
	"encoding/json/v2"
	"io"
	"net/http"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

type shutdownRun struct{ session, id, dir string }

func startShutdownRun(t *testing.T, s *shutdownServer, dir string, prompt ...string) shutdownRun {
	t.Helper()
	text := "gate request-model"
	if len(prompt) != 0 {
		text = prompt[0]
	}
	body, _ := json.Marshal(map[string]string{"prompt": text})
	res, err := http.Post(s.url+"/api/runs", "application/json", strings.NewReader(string(body)))
	if err != nil {
		t.Fatal(err)
	}
	defer res.Body.Close()
	var response struct {
		Session string `json:"session_id"`
		ID      string `json:"run_id"`
	}
	if err := json.UnmarshalRead(res.Body, &response); err != nil || res.StatusCode != http.StatusAccepted || response.Session == "" {
		t.Fatalf("start: %s %+v %v", res.Status, response, err)
	}
	r := shutdownRun{session: response.Session, id: response.ID, dir: filepath.Join(dir, "sessions", runsDir, response.ID)}
	t.Cleanup(func() {
		ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
		defer cancel()
		_ = cockpit.StopDetached(ctx, r.dir, "the shutdown control test ended")
	})
	waitFor(t, func() bool {
		tr, err := cockpit.LoadSession(filepath.Join(dir, "sessions"), r.session)
		return err == nil && len(tr.Entries) != 0
	})
	return r
}

func waitShutdownExit(t *testing.T, s *shutdownServer) {
	t.Helper()
	select {
	case <-s.done:
		if s.err != nil {
			data, _ := os.ReadFile(s.log)
			t.Fatalf("fixture exit: %v\n%s", s.err, data)
		}
	case <-time.After(15 * time.Second):
		data, _ := os.ReadFile(s.log)
		t.Fatalf("fixture did not exit\n%s", data)
	}
}

func shutdownFixture(t *testing.T) options {
	t.Helper()
	o := gatewayFixtureOptions(t)
	short := filepath.Join(o.Workspace, "accounts")
	if err := os.Rename(o.accountsDir, short); err != nil {
		t.Fatal(err)
	}
	o.accountsDir = short
	return o
}

func waitInteractivePrompt(t *testing.T, s *shutdownServer) {
	t.Helper()
	waitFor(t, func() bool {
		data, _ := os.ReadFile(s.log)
		return strings.Contains(string(data), "press Ctrl+C again within 5 seconds to stop it")
	})
}

func TestInteractiveShutdownKeepsAgentAndSecondControlCStopsIt(t *testing.T) {
	for _, second := range []bool{false, true} {
		name := "first-control-c"
		if second {
			name = "second-control-c"
		}
		t.Run(name, func(t *testing.T) {
			o := shutdownFixture(t)
			s := startShutdownServer(t, o.Workspace, o.accounts, true)
			prompt := "gate request-model"
			if second {
				prompt = "wait request-model"
			}
			run := startShutdownRun(t, s, o.Workspace, prompt)
			d, err := loadGatewayHost(o.accountsDir)
			if err != nil {
				t.Fatal(err)
			}
			s.sendInterrupt(t) // ^C through the PTY's line discipline, not kill(2).
			waitInteractivePrompt(t, s)
			if second {
				s.sendInterrupt(t)
			}
			waitShutdownExit(t, s)
			if second {
				waitFor(t, func() bool { return !cockpit.SessionBusy(o.SessionDir, run.session) })
				data, err := os.ReadFile(cockpit.SessionPath(o.SessionDir, run.session))
				if err != nil || !strings.Contains(string(data), "the user stopped the server and its agents") {
					t.Fatalf("second Ctrl+C did not record its deliberate stop: %s %v", data, err)
				}
				waitGatewayGone(t, newGatewayPeer(d, nil))
				return
			}
			if !cockpit.SessionBusy(o.SessionDir, run.session) {
				t.Fatal("first interactive Ctrl+C stopped the agent")
			}
			if err := os.WriteFile(filepath.Join(o.Workspace, "gate"), nil, 0o600); err != nil {
				t.Fatal(err)
			}
			waitFor(t, func() bool { return !cockpit.SessionBusy(o.SessionDir, run.session) })
			tr, err := cockpit.LoadSession(o.SessionDir, run.session)
			if err != nil || tr.Entries[len(tr.Entries)-1].Text != "echo: gate request-model" {
				t.Fatalf("offline model request after interactive Ctrl+C: %+v %v", tr, err)
			}
			waitGatewayGone(t, newGatewayPeer(d, nil))
			next := startShutdownServer(t, o.Workspace, o.accounts, true)
			waitFor(t, func() bool { _, err := os.Stat(run.dir); return os.IsNotExist(err) })
			next.interrupt(t)
		})
	}
}

func TestExplicitCancelStopsLiveAndAdoptedGatewayRuns(t *testing.T) {
	for _, adopt := range []bool{false, true} {
		name := "live"
		if adopt {
			name = "adopted"
		}
		t.Run(name, func(t *testing.T) {
			o := shutdownFixture(t)
			s := startShutdownServer(t, o.Workspace, o.accounts)
			run := startShutdownRun(t, s, o.Workspace, "wait request-model")
			if adopt {
				s.interrupt(t)
				s = startShutdownServer(t, o.Workspace, o.accounts)
				res, err := http.Post(s.url+"/api/runs", "application/json", strings.NewReader(`{"session_id":"`+run.session+`","prompt":"must not start"}`))
				if err != nil {
					t.Fatal(err)
				}
				var busy struct {
					RunID string `json:"run_id"`
				}
				err = json.UnmarshalRead(res.Body, &busy)
				res.Body.Close()
				if err != nil || res.StatusCode != http.StatusConflict || busy.RunID == "" {
					t.Fatalf("adopted run: %s %+v %v", res.Status, busy, err)
				}
				run.id = busy.RunID
			}
			res, err := http.Post(s.url+"/api/runs/"+run.id+"/cancel", "application/json", nil)
			if err != nil {
				t.Fatal(err)
			}
			body, _ := io.ReadAll(res.Body)
			res.Body.Close()
			if res.StatusCode != http.StatusAccepted {
				t.Fatalf("explicit cancel: %s %s", res.Status, body)
			}
			waitFor(t, func() bool { return !cockpit.SessionBusy(o.SessionDir, run.session) })
			data, err := os.ReadFile(cockpit.SessionPath(o.SessionDir, run.session))
			if err != nil || !strings.Contains(string(data), "the user stopped the run from the cockpit") || strings.Contains(string(data), "echo: gate request-model") {
				t.Fatalf("explicit cancel did not record the stop: %s %v", data, err)
			}
			d, err := loadGatewayHost(o.accountsDir)
			if err != nil {
				t.Fatal(err)
			}
			s.interrupt(t)
			waitGatewayGone(t, newGatewayPeer(d, nil))
		})
	}
}
