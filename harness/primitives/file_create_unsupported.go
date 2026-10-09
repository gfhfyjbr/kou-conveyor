//go:build !unix

package primitives

import (
	"context"
	"fmt"
)

func createNewPath(_ context.Context, request IOCreateRequest) (IOCreateResult, error) {
	return IOCreateResult{}, fmt.Errorf("create %q: unsupported platform", request.Path)
}
