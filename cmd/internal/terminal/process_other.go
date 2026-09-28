//go:build !unix

package terminal

import (
	"errors"
	"os"
)

func foreground(*os.File) int { return 0 }

func processDir(int) (string, error) { return "", errors.ErrUnsupported }

func hangUpGroup(int) {}

func killGroup(int) {}
