//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package cockpit

import (
	"bufio"
	"context"
	"errors"
	"fmt"
	"net"
	"net/http"
	"os"
	"syscall"
)

// The same publication may be shared by helpers with different accounts
// directories, so its compare/remove needs a lock on the publication itself.
func lockGatewayPublication(path string) (func(), error) {
	f, err := os.OpenFile(path+".lock", os.O_CREATE|os.O_RDWR, 0o600)
	if err != nil {
		return nil, err
	}
	for {
		err = syscall.Flock(int(f.Fd()), syscall.LOCK_EX)
		if !errors.Is(err, syscall.EINTR) {
			break
		}
	}
	if err != nil {
		f.Close()
		return nil, err
	}
	return func() { f.Close() }, nil
}

// Lease keeps a managed gateway alive until the returned file is closed. A
// detached runner inherits the file, so a front-end's exit does not release it.
// A gateway published by an older cockpit needs no lease.
func (g Gateway) Lease(ctx context.Context) (*os.File, error) {
	if g.LeaseSocket == "" {
		return nil, nil
	}
	conn, err := (&net.Dialer{}).DialContext(ctx, "unix", g.LeaseSocket)
	if err != nil {
		return nil, fmt.Errorf("lease the accounts gateway: %w", err)
	}
	defer conn.Close()
	stop := context.AfterFunc(ctx, func() { conn.Close() })
	defer stop()
	req, _ := http.NewRequest(http.MethodGet, "http://accounts/internal/lease", nil)
	req.Header.Set("Authorization", "Bearer "+g.APIKey)
	req.Header.Set("X-Kou-Gateway-Owner", g.Owner)
	if err := req.Write(conn); err != nil {
		return nil, err
	}
	res, err := http.ReadResponse(bufio.NewReader(conn), req)
	if err != nil {
		return nil, err
	}
	defer res.Body.Close()
	if res.StatusCode != http.StatusOK || res.Header.Get("X-Kou-Gateway-Owner") != g.Owner {
		return nil, fmt.Errorf("lease the accounts gateway: the helper did not confirm its owner")
	}
	file, err := conn.(*net.UnixConn).File()
	if err != nil {
		return nil, err
	}
	if ctx.Err() != nil {
		file.Close()
		return nil, ctx.Err()
	}
	return file, nil
}
