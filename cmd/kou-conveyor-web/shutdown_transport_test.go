//go:build unix

package main

import (
	"context"
	"encoding/json/v2"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"os"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"

	"github.com/creack/pty"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

const shutdownServerEnvironment = "KOU_TEST_SHUTDOWN_SERVER"

// The test binary serves the real cockpit in a process group of its own. Its
// runner is the same binary's fake-runner mode, not an installed program.
func TestShutdownTransportServer(t *testing.T) {
	raw := os.Getenv(shutdownServerEnvironment)
	if raw == "" {
		return
	}
	var args []string
	if err := json.Unmarshal([]byte(raw), &args); err != nil {
		t.Fatal(err)
	}
	args = append(args, "-runner", cockpittest.Runner(t))
	os.Exit(runMain(args))
}

type shutdownServer struct {
	url     string
	process *os.Process
	done    chan struct{}
	err     error
	log     string
	tty     *os.File
}

func shutdownAddress(t *testing.T) string {
	t.Helper()
	l, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer l.Close()
	return l.Addr().String()
}

func startShutdownServer(t *testing.T, dir, accounts string, interactive ...bool) *shutdownServer {
	t.Helper()
	return startShutdownServerWithArgs(t, dir, accounts, len(interactive) > 0 && interactive[0])
}

func startShutdownServerWithArgs(t *testing.T, dir, accounts string, interactive bool, extra ...string) *shutdownServer {
	t.Helper()
	address := shutdownAddress(t)
	args, err := json.Marshal(append([]string{
		"-address", address, "-workspace", dir, "-session-directory", filepath.Join(dir, "sessions"),
		"-config", filepath.Join(dir, "settings.json"), "-accounts", accounts,
		"-accounts-dir", filepath.Join(dir, "accounts"), "-rebuild=false", "-canvas=off", "-tunnel=off", "-assets=embedded",
	}, extra...))
	if err != nil {
		t.Fatal(err)
	}
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	log, err := os.CreateTemp(dir, "server-*.log")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() { log.Close() })
	cmd := spawn.Command(exe, "-test.run=^TestShutdownTransportServer$")
	cmd.Dir = dir
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	for _, entry := range os.Environ() {
		name, _, _ := strings.Cut(entry, "=")
		if !strings.HasPrefix(name, "KOU_") && name != "COCKPITTEST_FAKE_RUNNER" && name != "HOME" && !strings.HasPrefix(name, "XDG_") {
			cmd.Env = append(cmd.Env, entry)
		}
	}
	cmd.Env = append(cmd.Env, shutdownServerEnvironment+"="+string(args), "HOME="+dir,
		"XDG_CONFIG_HOME="+filepath.Join(dir, "config"), "XDG_CACHE_HOME="+filepath.Join(dir, "cache"))
	cmd.Stdout, cmd.Stderr = log, log
	var master, slave *os.File
	if interactive {
		master, slave, err = pty.Open()
		if err != nil {
			t.Fatal(err)
		}
		defer slave.Close()
		t.Cleanup(func() { master.Close() })
		cmd.Stdin = slave
		cmd.SysProcAttr.Setctty = true
	}
	if err := cmd.Start(); err != nil {
		t.Fatal(err)
	}
	s := &shutdownServer{url: "http://" + address, process: cmd.Process, done: make(chan struct{}), log: log.Name(), tty: master}
	go func() {
		s.err = cmd.Wait()
		close(s.done)
	}()
	t.Cleanup(func() {
		select {
		case <-s.done:
		default:
			_ = syscall.Kill(-s.process.Pid, syscall.SIGKILL)
			<-s.done
		}
	})
	waitFor(t, func() bool {
		select {
		case <-s.done:
			data, _ := os.ReadFile(s.log)
			t.Fatalf("fixture exited: %v\n%s", s.err, data)
		default:
		}
		res, err := http.Get(s.url + "/api/config")
		if err != nil {
			return false
		}
		res.Body.Close()
		if res.StatusCode != http.StatusOK {
			return false
		}
		if accounts != "off" {
			g, err := cockpit.LoadGateway(cockpit.GatewayPath(filepath.Join(dir, "settings.json")))
			return err == nil && g.Reach(t.Context()) == nil
		}
		return true
	})
	return s
}

func (s *shutdownServer) sendInterrupt(t *testing.T) {
	t.Helper()
	if s.tty != nil {
		if _, err := s.tty.Write([]byte{3}); err != nil {
			t.Fatal(err)
		}
	} else if err := syscall.Kill(-s.process.Pid, syscall.SIGINT); err != nil {
		t.Fatal(err)
	}
}

func (s *shutdownServer) interrupt(t *testing.T) {
	t.Helper()
	s.sendInterrupt(t)
	select {
	case <-s.done:
		if s.err != nil {
			data, _ := os.ReadFile(s.log)
			t.Fatalf("fixture shutdown: %v\n%s", s.err, data)
		}
	case <-time.After(15 * time.Second):
		data, _ := os.ReadFile(s.log)
		t.Fatalf("fixture did not exit after Ctrl+C\n%s", data)
	}
}

func TestDetachedModelTransportSurvivesShutdown(t *testing.T) {
	for _, gateway := range []bool{false, true} {
		t.Run(fmt.Sprintf("gateway=%t", gateway), func(t *testing.T) {
			dir := t.TempDir()
			requests := make(chan struct{}, 2)
			upstream := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
				if r.URL.Path != "/v1/chat/completions" {
					http.NotFound(w, r)
					return
				}
				requests <- struct{}{}
				w.Header().Set("Content-Type", "application/json")
				io.WriteString(w, `{"id":"c1","object":"chat.completion","created":1,"model":"fake-model","choices":[{"index":0,"message":{"role":"assistant","content":"hi"},"finish_reason":"stop"}],"usage":{"prompt_tokens":1,"completion_tokens":1,"total_tokens":2}}`)
			}))
			defer upstream.Close()
			settings := cockpit.Settings{API: cockpit.APIResponses, BaseURL: upstream.URL + "/v1", APIKey: "sk-fixture", Model: "fake-model"}
			address := "off"
			if gateway {
				address = shutdownAddress(t)
				settings = cockpit.Settings{API: cockpit.APIGateway, Model: "fake-model"}
				if err := os.MkdirAll(filepath.Join(dir, "accounts"), 0o700); err != nil {
					t.Fatal(err)
				}
				config := fmt.Sprintf("openai-compatibility:\n  - name: fakeai\n    base-url: %q\n    api-key-entries:\n      - api-key: sk-fixture\n    models:\n      - name: fake-model\n        alias: fake-model\n", upstream.URL+"/v1")
				if err := os.WriteFile(filepath.Join(dir, "accounts", "config.yaml"), []byte(config), 0o600); err != nil {
					t.Fatal(err)
				}
			}
			settingsFile := filepath.Join(dir, "settings.json")
			if err := cockpit.SaveSettings(settingsFile, settings); err != nil {
				t.Fatal(err)
			}
			if err := cockpit.SaveAutoTitles(cockpit.PreferencesPath(settingsFile), false); err != nil {
				t.Fatal(err)
			}
			server := startShutdownServer(t, dir, address)
			res, err := http.Post(server.url+"/api/runs", "application/json", strings.NewReader(`{"prompt":"gate request-model"}`))
			if err != nil {
				t.Fatal(err)
			}
			var started struct {
				SessionID string `json:"session_id"`
				RunID     string `json:"run_id"`
			}
			data, _ := io.ReadAll(res.Body)
			res.Body.Close()
			if res.StatusCode != http.StatusAccepted || json.Unmarshal(data, &started) != nil || started.SessionID == "" {
				t.Fatalf("start: %d %s", res.StatusCode, data)
			}
			sessions := filepath.Join(dir, "sessions")
			runDir := filepath.Join(sessions, runsDir, started.RunID)
			t.Cleanup(func() {
				ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
				defer cancel()
				_ = cockpit.StopDetached(ctx, runDir, "the shutdown transport test ended")
			})
			waitFor(t, func() bool {
				tr, err := cockpit.LoadSession(sessions, started.SessionID)
				return err == nil && len(tr.Entries) != 0
			})
			info, err := cockpit.ReadDetachedRun(runDir)
			if err != nil {
				t.Fatal(err)
			}
			server.interrupt(t)
			if err := syscall.Kill(info.PID, 0); err != nil || !cockpit.SessionBusy(sessions, started.SessionID) {
				t.Fatalf("runner did not survive first Ctrl+C: pid=%d err=%v", info.PID, err)
			}
			t.Logf("server PID %d exited; runner PID %d still holds its session", server.process.Pid, info.PID)
			job, _, err := cockpit.Adopt(t.Context(), runDir)
			if err != nil {
				t.Fatal(err)
			}
			lines := make(chan []cockpit.Line, 1)
			go func() {
				var got []cockpit.Line
				for line := range job.Lines() {
					got = append(got, line)
				}
				lines <- got
			}()
			if err := os.WriteFile(filepath.Join(dir, "gate"), nil, 0o600); err != nil {
				t.Fatal(err)
			}
			select {
			case got := <-lines:
				if job.Err() != nil {
					t.Fatalf("model request after server exit stopped the surviving runner: %v\n%+v", job.Err(), got)
				}
			case <-time.After(10 * time.Second):
				t.Fatal("runner did not finish its offline model request")
			}
			select {
			case <-requests:
			default:
				t.Fatal("runner finished without reaching the local model")
			}
			next := startShutdownServer(t, dir, address)
			waitFor(t, func() bool {
				_, err := os.Stat(runDir)
				return os.IsNotExist(err)
			})
			tr, err := cockpit.LoadSession(sessions, started.SessionID)
			if err != nil || tr.Entries[len(tr.Entries)-1].Text != "echo: gate request-model" {
				t.Fatalf("restart did not take up the answer: %+v %v", tr, err)
			}
			next.interrupt(t)
		})
	}
}
