//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package main

import (
	"context"
	"crypto/rand"
	"crypto/subtle"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"path/filepath"
	"sync"
	"syscall"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/internal/spawn"
)

const gatewayHostAvailable = true

// The short-lived start lock serializes attach/create with the last lease's
// drain. The inherited owner lock belongs to one helper for its whole lifetime.
func gatewayHostLock(dir, name string, nonblock bool) (*os.File, error) {
	f, err := os.OpenFile(filepath.Join(dir, name+".lock"), os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	flags := syscall.LOCK_EX
	if nonblock {
		flags |= syscall.LOCK_NB
	}
	for {
		err = syscall.Flock(int(f.Fd()), flags)
		if !errors.Is(err, syscall.EINTR) {
			break
		}
	}
	if err != nil {
		f.Close()
		return nil, err
	}
	return f, nil
}

func ownEmbeddedGateway(dir string) (func(), error) {
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, err
	}
	start, err := gatewayHostLock(dir, "host-start", false)
	if err != nil {
		return nil, err
	}
	defer start.Close()
	owner, err := gatewayHostLock(dir, "host-owner", true)
	if err != nil {
		return nil, fmt.Errorf("the accounts directory %s is already owned by another server; stop it or use another -accounts-dir: %w", dir, err)
	}
	return func() { owner.Close() }, nil
}

func startGatewayPeer(ctx context.Context, o options) (*gatewayPeer, error) {
	if err := os.MkdirAll(o.accountsDir, 0o700); err != nil {
		return nil, err
	}
	start, err := gatewayHostLock(o.accountsDir, "host-start", false)
	if err != nil {
		return nil, err
	}
	defer start.Close()
	if d, err := loadGatewayHost(o.accountsDir); err == nil && d.Gateway.URL != "" {
		if lease, err := d.Gateway.Lease(ctx); err == nil {
			p := newGatewayPeer(d, lease)
			var status accounts.Status
			if err := p.request(ctx, "GET", "/internal/status", nil, &status); err != nil {
				lease.Close()
				return nil, err
			}
			if d.Address != o.accounts {
				lease.Close()
				return nil, fmt.Errorf("the accounts gateway is still serving clients at %s; use -accounts %s until they finish", d.Address, d.Address)
			}
			if err := p.publish(ctx, o.SettingsFile); err != nil {
				lease.Close()
				return nil, err
			}
			return p, nil
		}
	}
	owner, err := gatewayHostLock(o.accountsDir, "host-owner", true)
	if err != nil {
		return nil, fmt.Errorf("the accounts host still owns its files but did not answer; stop that server or use another -accounts-dir: %w", err)
	}
	defer owner.Close()
	d := gatewayHostDescription{Dir: o.accountsDir, Address: o.accounts, Token: rand.Text(), Gateway: cockpit.Gateway{Owner: rand.Text()}}
	if err := saveGatewayHost(d); err != nil {
		return nil, err
	}
	keepLease := false
	defer func() {
		if !keepLease {
			if current, err := loadGatewayHost(d.Dir); err == nil && current.Gateway.Owner == d.Gateway.Owner {
				os.Remove(gatewayHostPath(d.Dir))
			}
		}
	}()
	ready, reply, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	defer ready.Close()
	defer reply.Close()
	bootstrap, lease, err := os.Pipe()
	if err != nil {
		return nil, err
	}
	defer bootstrap.Close()
	defer func() {
		if !keepLease {
			lease.Close()
		}
	}()
	exe, err := os.Executable()
	if err != nil {
		return nil, err
	}
	if err := os.MkdirAll(filepath.Join(o.accountsDir, "logs"), 0o700); err != nil {
		return nil, err
	}
	log, err := os.OpenFile(filepath.Join(o.accountsDir, "logs", "host.log"), os.O_WRONLY|os.O_CREATE|os.O_APPEND, 0o600)
	if err != nil {
		return nil, err
	}
	defer log.Close()
	cmd := spawn.Command(exe, gatewayHostMode, gatewayHostPath(o.accountsDir))
	cmd.Stdout, cmd.Stderr = log, log
	cmd.SysProcAttr = &syscall.SysProcAttr{Setsid: true}
	cmd.ExtraFiles = []*os.File{owner, reply, bootstrap}
	if err := cmd.Start(); err != nil {
		return nil, err
	}
	go func() { _ = cmd.Wait() }()
	// Only the child writes readiness. Only this web process owns the write
	// end of the bootstrap lease, including if it dies before readiness.
	reply.Close()
	bootstrap.Close()
	stop := context.AfterFunc(ctx, func() { ready.Close() })
	defer stop()
	if err := json.UnmarshalRead(ready, &d); err != nil {
		return nil, fmt.Errorf("start the accounts host: %w (see %s)", err, log.Name())
	}
	p := newGatewayPeer(d, lease)
	if err := p.publish(ctx, o.SettingsFile); err != nil {
		return nil, err
	}
	keepLease = true
	return p, nil
}

func (p *gatewayPeer) publish(ctx context.Context, settings string) error {
	if settings == "" {
		return nil
	}
	return p.request(ctx, "POST", "/internal/publish", map[string]string{"path": cockpit.GatewayPath(settings)}, nil)
}

type gatewayLeases struct {
	mu      sync.Mutex
	dir     string
	count   int
	closing bool
	idle    chan struct{}
	drain   *os.File
}

func (l *gatewayLeases) release() {
	l.mu.Lock()
	l.count--
	empty := l.count == 0
	l.mu.Unlock()
	if !empty {
		return
	}
	start, err := gatewayHostLock(l.dir, "host-start", false)
	if err != nil {
		l.mu.Lock()
		if l.count == 0 && !l.closing {
			l.closing = true
			close(l.idle)
		}
		l.mu.Unlock()
		return
	}
	l.mu.Lock()
	defer l.mu.Unlock()
	if l.count == 0 && !l.closing {
		l.closing, l.drain = true, start
		close(l.idle)
	} else {
		start.Close()
	}
}

func (l *gatewayLeases) serve(w http.ResponseWriter, r *http.Request, owner string) {
	l.mu.Lock()
	if l.closing {
		l.mu.Unlock()
		writeError(w, http.StatusServiceUnavailable, "the accounts host is draining")
		return
	}
	l.count++
	l.mu.Unlock()
	conn, rw, err := w.(http.Hijacker).Hijack()
	if err != nil {
		l.release()
		return
	}
	defer conn.Close()
	defer l.release()
	fmt.Fprintf(rw, "HTTP/1.1 200 OK\r\nContent-Length: 0\r\nX-Kou-Gateway-Owner: %s\r\n\r\n", owner)
	if err := rw.Flush(); err != nil {
		return
	}
	_, _ = io.Copy(io.Discard, rw)
}

func runGatewayHost(path string) int {
	owner := os.NewFile(3, "accounts owner lock")
	ready := os.NewFile(4, "accounts readiness")
	bootstrap := os.NewFile(5, "accounts bootstrap lease")
	defer owner.Close()
	defer ready.Close()
	defer bootstrap.Close()
	for _, fd := range []int{3, 4, 5} {
		syscall.CloseOnExec(fd)
	}
	d, err := loadGatewayHost(filepath.Dir(path))
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	defer func() {
		if current, err := loadGatewayHost(d.Dir); err == nil && current.Gateway.Owner == d.Gateway.Owner {
			os.Remove(gatewayHostPath(d.Dir))
		}
	}()
	// /tmp keeps sockaddr_un short even with a long user config directory or
	// Darwin's long per-user TMPDIR. Only this helper owns this private folder.
	socketDir, err := os.MkdirTemp("/tmp", "kou-accounts-")
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	defer os.RemoveAll(socketDir)
	d.Gateway.LeaseSocket = filepath.Join(socketDir, "control.sock")
	leases := &gatewayLeases{dir: d.Dir, count: 1, idle: make(chan struct{})}
	go func() {
		_, _ = io.Copy(io.Discard, bootstrap)
		leases.release()
	}()
	ctx, cancel := context.WithCancel(context.Background())
	defer cancel()
	host, err := accounts.New(accounts.Options{Dir: d.Dir, Address: d.Address})
	if err != nil {
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	done := host.Done()
	go host.Run(ctx)
	select {
	case <-host.Ready():
	case <-done:
		fmt.Fprintln(os.Stderr, host.Status().Error)
		return 1
	case <-leases.idle:
		cancel()
		<-done
		return 0
	}
	d.Gateway.URL, d.Gateway.APIKey = host.URL(), host.APIKey()
	listener, err := net.Listen("unix", d.Gateway.LeaseSocket)
	if err != nil {
		cancel()
		<-done
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	defer listener.Close()
	if err := os.Chmod(d.Gateway.LeaseSocket, 0o600); err != nil {
		cancel()
		<-done
		fmt.Fprintln(os.Stderr, err)
		return 1
	}
	s := &server{ctx: ctx, gateway: host}
	mux := http.NewServeMux()
	s.gatewayRoutes(mux)
	mux.HandleFunc("GET /api/accounts", s.handleAccounts)
	mux.HandleFunc("GET /internal/status", func(w http.ResponseWriter, r *http.Request) { writeJSON(w, http.StatusOK, host.Status()) })
	mux.HandleFunc("GET /internal/lease", func(w http.ResponseWriter, r *http.Request) { leases.serve(w, r, d.Gateway.Owner) })
	mux.HandleFunc("POST /internal/run-environment", func(w http.ResponseWriter, r *http.Request) {
		var request struct {
			Model string `json:"model"`
		}
		if !decodeJSON(w, r, &request) {
			return
		}
		writeJSON(w, http.StatusOK, gatewayRunEnvironment{
			Env:          append(s.contextEnv(request.Model), s.cacheEnv(request.Model)...),
			ServedWindow: host.ServedContextWindow(request.Model),
		})
	})
	var publicationsMu sync.Mutex
	publications := map[string]bool{}
	mux.HandleFunc("POST /internal/publish", func(w http.ResponseWriter, r *http.Request) {
		var request struct {
			Path string `json:"path"`
		}
		if !decodeJSON(w, r, &request) {
			return
		}
		if !filepath.IsAbs(request.Path) || filepath.Base(request.Path) != "gateway.json" {
			writeError(w, http.StatusBadRequest, "a gateway publication needs an absolute gateway.json path")
			return
		}
		publicationsMu.Lock()
		defer publicationsMu.Unlock()
		if err := cockpit.PublishGateway(request.Path, d.Gateway); err != nil {
			writeError(w, http.StatusInternalServerError, err.Error())
			return
		}
		publications[request.Path] = true
		writeJSON(w, http.StatusOK, map[string]bool{"published": true})
	})
	auth := http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		key := d.Token
		if r.URL.Path == "/internal/lease" {
			key = d.Gateway.APIKey
		}
		if subtle.ConstantTimeCompare([]byte(r.Header.Get("Authorization")), []byte("Bearer "+key)) != 1 || r.Header.Get("X-Kou-Gateway-Owner") != d.Gateway.Owner {
			writeError(w, http.StatusUnauthorized, "the accounts host credentials do not match")
			return
		}
		mux.ServeHTTP(w, r)
	})
	web := &http.Server{Handler: auth}
	go func() { _ = web.Serve(listener) }()
	if err := saveGatewayHost(d); err != nil {
		fmt.Fprintln(os.Stderr, err)
		cancel()
	} else if err := json.MarshalWrite(ready, d); err != nil {
		cancel()
	}
	ready.Close()
	select {
	case <-leases.idle:
		cancel()
		<-done
	case <-done:
	case <-ctx.Done():
		<-done
	}
	web.Close()
	// Serializing cleanup with startup prevents a replacement from taking the
	// same endpoint/files until the old host and its writers have all stopped.
	leases.mu.Lock()
	start := leases.drain
	leases.closing = true
	leases.mu.Unlock()
	if start == nil {
		start, err = gatewayHostLock(d.Dir, "host-start", false)
		if err != nil {
			fmt.Fprintln(os.Stderr, err)
			return 1
		}
	}
	publicationsMu.Lock()
	for path := range publications {
		cockpit.WithdrawGateway(path, d.Gateway)
	}
	publicationsMu.Unlock()
	if current, err := loadGatewayHost(d.Dir); err == nil && current.Gateway.Owner == d.Gateway.Owner {
		os.Remove(gatewayHostPath(d.Dir))
	}
	owner.Close()
	start.Close()
	if status := host.Status(); status.State == accounts.GatewayFailed {
		fmt.Fprintln(os.Stderr, status.Error)
		return 1
	}
	return 0
}
