//go:build !darwin && !linux && !freebsd && !openbsd && !netbsd && !dragonfly

package main

import (
	"context"
	"errors"
)

const gatewayHostAvailable = false

func ownEmbeddedGateway(string) (func(), error) { return func() {}, nil }

func startGatewayPeer(context.Context, options) (*gatewayPeer, error) {
	return nil, errors.New("a detached accounts host is not supported on this platform")
}

func runGatewayHost(string) int { return 1 }
