//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package main

import (
	"context"
	"encoding/json/v2"
	"errors"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"syscall"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit/cockpittest"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/runconfig"
	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

func uniqueFakeRunner(t *testing.T) string {
	t.Helper()
	exe := cockpittest.Runner(t)
	path := filepath.Join(t.TempDir(), "runner")
	if err := os.Link(exe, path); err != nil {
		t.Fatal(err)
	}
	return path
}

func TestManagedStartKeepsTheResolvedGenerationAfterPublicationChanges(t *testing.T) {
	for _, replace := range []bool{false, true} {
		name := "withdrawn"
		if replace {
			name = "replaced"
		}
		t.Run(name, func(t *testing.T) {
			o := gatewayFixtureOptions(t)
			p := startFixturePeer(t, o)
			probeDir, err := os.MkdirTemp("/tmp", "kou-lease-test-")
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { os.RemoveAll(probeDir) })
			publishedGateway := p.description.Gateway
			publishedGateway.LeaseSocket = filepath.Join(probeDir, "lease.sock")
			listener, err := net.Listen("unix", publishedGateway.LeaseSocket)
			if err != nil {
				t.Fatal(err)
			}
			t.Cleanup(func() { listener.Close() })
			accepted := make(chan struct{}, 1)
			go func() {
				conn, err := listener.Accept()
				if err != nil {
					return
				}
				defer conn.Close()
				upstream, err := net.Dial("unix", p.description.Gateway.LeaseSocket)
				if err != nil {
					return
				}
				defer upstream.Close()
				accepted <- struct{}{}
				go func() { io.Copy(upstream, conn); upstream.Close() }()
				io.Copy(conn, upstream)
			}()
			if err := cockpit.PublishGateway(cockpit.GatewayPath(o.SettingsFile), publishedGateway); err != nil {
				t.Fatal(err)
			}
			o.Runner = uniqueFakeRunner(t)
			if err := cockpit.SaveSettings(o.SettingsFile, cockpit.Settings{API: cockpit.APIGateway, Model: "claude-fixture"}); err != nil {
				t.Fatal(err)
			}
			gate := filepath.Join(t.TempDir(), "providers")
			if err := syscall.Mkfifo(gate, 0o600); err != nil {
				t.Fatal(err)
			}
			t.Setenv(cockpittest.ProviderGate, gate)
			release := func() {
				f, err := os.OpenFile(gate, os.O_RDWR, 0)
				if err == nil {
					f.Close()
				}
			}
			t.Cleanup(release)
			type result struct {
				job *cockpit.Job
				err error
			}
			started := make(chan result, 1)
			ctx, cancel := context.WithCancel(t.Context())
			defer cancel()
			runDir := filepath.Join(o.SessionDir, runsDir, "pinned")
			t.Cleanup(func() {
				stop, end := context.WithTimeout(context.Background(), 10*time.Second)
				defer end()
				_ = cockpit.StopDetached(stop, runDir, "the test ended")
			})
			go func() {
				job, err := cockpit.Start(ctx, o.Options, cockpit.Request{SessionID: "pinned", MessageID: uuid.New().String(), Prompt: "wait for explicit stop", Detached: runDir})
				started <- result{job, err}
			}()
			waitFor(t, func() bool { _, err := os.Stat(gate + ".ready"); return err == nil })
			publication := cockpit.GatewayPath(o.SettingsFile)
			if replace {
				other := p.description.Gateway
				other.Owner, other.URL = "replacement", "http://127.0.0.1:1"
				if err := cockpit.PublishGateway(publication, other); err != nil {
					t.Fatal(err)
				}
			} else {
				cockpit.WithdrawGateway(publication, publishedGateway)
			}
			release()
			var got result
			select {
			case got = <-started:
			case <-time.After(10 * time.Second):
				t.Fatal("Start did not leave the provider probe")
			}
			if got.err != nil {
				t.Fatal(got.err)
			}
			select {
			case <-accepted:
			default:
				t.Fatal("the resolved gateway generation was launched without acquiring its lease")
			}
			go func() {
				for range got.job.Lines() {
				}
			}()
			waitFor(t, func() bool {
				tr, err := cockpit.LoadSession(o.SessionDir, "pinned")
				return err == nil && len(tr.Entries) > 0
			})
			cancel()
			<-got.job.Done()
			p.lease.Close()
			fixturePeerRequest(t, p, "GET", "/internal/status", nil, nil)
			stop, end := context.WithTimeout(t.Context(), 10*time.Second)
			defer end()
			if err := cockpit.StopDetached(stop, runDir, "the test ended"); err != nil {
				t.Fatal(err)
			}
			waitGatewayGone(t, p)
			if replace {
				current, err := cockpit.LoadGateway(publication)
				if err != nil || current.Owner != "replacement" {
					t.Fatalf("old cleanup removed the successor: %+v %v", current, err)
				}
			}
		})
	}
}

func TestManagedStartRejectsOldRunnerWithoutLeakingLease(t *testing.T) {
	o := gatewayFixtureOptions(t)
	p := startFixturePeer(t, o)
	o.Runner = uniqueFakeRunner(t)
	t.Setenv(cockpittest.NoGatewayLease, "1")
	_, err := cockpit.Start(t.Context(), o.Options, cockpit.Request{SessionID: "old", MessageID: uuid.New().String(), Prompt: "hello", Detached: filepath.Join(o.SessionDir, runsDir, "old")})
	if !errors.Is(err, cockpit.ErrRunnerOutdated) || !strings.Contains(err.Error(), "-gateway-lease") {
		t.Fatalf("old runner was not rejected with an update instruction: %v", err)
	}
	p.lease.Close()
	waitGatewayGone(t, p)
}

func TestSharedPublicationSurvivesOtherHostsCleanup(t *testing.T) {
	a, b := gatewayFixtureOptions(t), gatewayFixtureOptions(t)
	first := startFixturePeer(t, a)
	b.SettingsFile = a.SettingsFile
	second := startFixturePeer(t, b)
	var wgResult = make(chan error, 2)
	for _, p := range []*gatewayPeer{first, second} {
		go func() {
			for i := 0; i < 20; i++ {
				if err := p.publish(t.Context(), a.SettingsFile); err != nil {
					wgResult <- err
					return
				}
			}
			wgResult <- nil
		}()
	}
	for i := 0; i < 2; i++ {
		if err := <-wgResult; err != nil {
			t.Fatal(err)
		}
	}
	if err := second.publish(t.Context(), a.SettingsFile); err != nil {
		t.Fatal(err)
	}
	first.lease.Close()
	waitGatewayGone(t, first)
	current, err := cockpit.LoadGateway(cockpit.GatewayPath(a.SettingsFile))
	if err != nil || current.Owner != second.description.Gateway.Owner {
		t.Fatalf("first host removed the shared successor publication: %+v %v", current, err)
	}
	second.lease.Close()
	waitGatewayGone(t, second)
}

func TestLastLeaseDrainsWhenStartLockCannotBeOpened(t *testing.T) {
	dir := filepath.Join(t.TempDir(), "missing")
	l := &gatewayLeases{dir: dir, count: 1, idle: make(chan struct{})}
	l.release()
	select {
	case <-l.idle:
	default:
		t.Fatal("zero leases and a missing lock directory left the host alive")
	}
}

func TestGatewayRunEnvironmentSurvivesHelperLossForUnrelatedProviders(t *testing.T) {
	o := gatewayFixtureOptions(t)
	p := startFixturePeer(t, o)
	fixturePeerRequest(t, p, "POST", "/api/context-windows", map[string]any{"model": "direct-model", "tokens": 64000}, nil)
	fixturePeerRequest(t, p, "POST", "/api/cache-ttls", map[string]string{"model": "direct-model", "ttl": "1h"}, nil)
	p.lease.Close()
	waitGatewayGone(t, p)
	s := &server{ctx: t.Context(), gatewayPeer: p}
	env, err := s.gatewayRunEnv(t.Context(), "direct-model")
	if err != nil || !slices.Equal(env, []string{runconfig.ContextWindowEnvironment + "=64000", runconfig.CacheTTLEnvironment + "=1h"}) {
		t.Fatalf("unrelated provider lost its persisted overrides: %v %v", env, err)
	}
}

type gatewayTestTransport func(*http.Request) (*http.Response, error)

func (f gatewayTestTransport) RoundTrip(r *http.Request) (*http.Response, error) { return f(r) }

func TestPrivateGatewayTransportAcceptsSuccessResponses(t *testing.T) {
	for _, status := range []int{http.StatusOK, http.StatusCreated, http.StatusNoContent} {
		p := &gatewayPeer{client: &http.Client{Transport: gatewayTestTransport(func(r *http.Request) (*http.Response, error) {
			body := `{"ok":true}`
			if status == http.StatusNoContent {
				body = ""
			}
			return &http.Response{StatusCode: status, Header: make(http.Header), Body: io.NopCloser(strings.NewReader(body))}, nil
		})}}
		var response map[string]any
		if err := p.request(t.Context(), "POST", "/api/endpoints", nil, &response); err != nil {
			t.Fatalf("status %d: %v", status, err)
		}
	}
}

func TestGatewayBootstrapLeaseCleansUpBeforeAndAfterReadiness(t *testing.T) {
	for _, beforeReady := range []bool{true, false} {
		name := "after-ready"
		if beforeReady {
			name = "before-ready"
		}
		t.Run(name, func(t *testing.T) {
			o := gatewayFixtureOptions(t)
			d := gatewayHostDescription{Dir: o.accountsDir, Address: o.accounts, Token: "bootstrap-private-token", Gateway: cockpit.Gateway{Owner: "bootstrap-generation"}}
			if err := saveGatewayHost(d); err != nil {
				t.Fatal(err)
			}
			owner, err := gatewayHostLock(d.Dir, "host-owner", true)
			if err != nil {
				t.Fatal(err)
			}
			defer owner.Close()
			ready, response, err := os.Pipe()
			if err != nil {
				t.Fatal(err)
			}
			defer ready.Close()
			defer response.Close()
			bootstrap, lease, err := os.Pipe()
			if err != nil {
				t.Fatal(err)
			}
			defer bootstrap.Close()
			defer lease.Close()
			if beforeReady {
				lease.Close()
			}
			exe, err := os.Executable()
			if err != nil {
				t.Fatal(err)
			}
			cmd := spawn.CommandContext(t.Context(), exe, gatewayHostMode, gatewayHostPath(d.Dir))
			cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
			cmd.ExtraFiles = []*os.File{owner, response, bootstrap}
			if err := cmd.Start(); err != nil {
				t.Fatal(err)
			}
			owner.Close()
			response.Close()
			bootstrap.Close()
			if !beforeReady {
				if err := json.UnmarshalRead(ready, &d); err != nil {
					t.Fatal(err)
				}
				lease.Close()
			}
			if err := cmd.Wait(); err != nil {
				t.Fatal(err)
			}
			if _, err := os.Stat(gatewayHostPath(d.Dir)); !os.IsNotExist(err) {
				t.Fatalf("orphan descriptor: %v", err)
			}
			if d.Gateway.LeaseSocket != "" {
				if _, err := os.Stat(d.Gateway.LeaseSocket); !os.IsNotExist(err) {
					t.Fatalf("orphan socket: %v", err)
				}
			}
			lock, err := gatewayHostLock(d.Dir, "host-owner", true)
			if err != nil {
				t.Fatal("orphan owner lock:", err)
			}
			lock.Close()
		})
	}
}

func TestGatewayStartupFailureLeavesNoOwner(t *testing.T) {
	o := gatewayFixtureOptions(t)
	o.accounts = "127.0.0.1:not-a-port"
	if p, err := startGatewayPeer(t.Context(), o); err == nil {
		p.lease.Close()
		t.Fatal("invalid gateway address started")
	}
	waitFor(t, func() bool {
		lock, err := gatewayHostLock(o.accountsDir, "host-owner", true)
		if err != nil {
			return false
		}
		lock.Close()
		_, err = os.Stat(gatewayHostPath(o.accountsDir))
		return os.IsNotExist(err)
	})
}
