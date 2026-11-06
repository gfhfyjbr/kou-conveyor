//go:build darwin || linux || freebsd || openbsd || netbsd || dragonfly

package main

import (
	"context"
	"strings"
	"testing"
)

func TestGatewayMixedModesKeepOneAccountsOwner(t *testing.T) {
	t.Run("embedded-first", func(t *testing.T) {
		o := gatewayFixtureOptions(t)
		embedded := startShutdownServerWithArgs(t, o.Workspace, o.accounts, false, "-detach=false", "-accounts-dir", o.accountsDir)
		o.accounts = shutdownAddress(t)
		p, err := startGatewayPeer(t.Context(), o)
		if err == nil {
			p.lease.Close()
			waitGatewayGone(t, p)
			t.Error("a helper started a second accounts writer beside an embedded gateway")
		} else if !strings.Contains(err.Error(), "owns its files") {
			t.Errorf("the ownership conflict was not explained: %v", err)
		}
		embedded.interrupt(t)
		p = startFixturePeer(t, o)
		p.lease.Close()
		waitGatewayGone(t, p)
	})

	t.Run("managed-first", func(t *testing.T) {
		o := gatewayFixtureOptions(t)
		p := startFixturePeer(t, o)
		ctx, cancel := context.WithCancel(t.Context())
		defer cancel()
		embedded := &server{opt: o}
		embedded.opt.detach = false
		embedded.startGateway(ctx)
		t.Cleanup(func() {
			cancel()
			if embedded.gatewayPeer != nil {
				embedded.gatewayPeer.lease.Close()
			}
			if embedded.gatewayGone != nil {
				<-embedded.gatewayGone
			}
		})
		if embedded.hasGateway() || !strings.Contains(embedded.gatewayErr, "already owned") {
			t.Errorf("an embedded startup did not reject the existing owner: host=%t peer=%t error=%q", embedded.gateway != nil, embedded.gatewayPeer != nil, embedded.gatewayErr)
		}
		if embedded.gatewayPeer != nil {
			embedded.gatewayPeer.lease.Close()
		}
		p.lease.Close()
		waitGatewayGone(t, p)
		restarted := startShutdownServerWithArgs(t, o.Workspace, embedded.opt.accounts, false, "-detach=false", "-accounts-dir", o.accountsDir)
		restarted.interrupt(t)
		p = startFixturePeer(t, o)
		p.lease.Close()
		waitGatewayGone(t, p)
	})
}

func TestEmbeddedGatewayConstructorFailureReleasesOwner(t *testing.T) {
	o := gatewayFixtureOptions(t)
	broken := &server{opt: o}
	broken.opt.detach = false
	broken.opt.accounts = "invalid-address"
	broken.startGateway(t.Context())
	if broken.hasGateway() || broken.gatewayErr == "" {
		t.Fatalf("invalid embedded address: host=%t error=%q", broken.hasGateway(), broken.gatewayErr)
	}
	owner, err := gatewayHostLock(o.accountsDir, "host-owner", true)
	if err != nil {
		t.Fatalf("a failed constructor kept the accounts owner: %v", err)
	}
	owner.Close()
	p := startFixturePeer(t, o)
	p.lease.Close()
	waitGatewayGone(t, p)
}
