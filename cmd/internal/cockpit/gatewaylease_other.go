//go:build !darwin && !linux && !freebsd && !openbsd && !netbsd && !dragonfly

package cockpit

import (
	"context"
	"os"
	"sync"
)

var gatewayPublicationMu sync.Mutex

func lockGatewayPublication(string) (func(), error) {
	gatewayPublicationMu.Lock()
	return gatewayPublicationMu.Unlock, nil
}

func (g Gateway) Lease(context.Context) (*os.File, error) { return nil, nil }
