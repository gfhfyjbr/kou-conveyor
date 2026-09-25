//go:build unix && !darwin

package primitives

import (
	"fmt"
	"os"
)

func syncCreatedFile(file *os.File, parent *os.File) error {
	if err := file.Sync(); err != nil {
		return fmt.Errorf("sync file: %w", err)
	}
	return syncCreatedDirectory(parent)
}

func syncCreatedDirectory(parent *os.File) error {
	if err := parent.Sync(); err != nil {
		return fmt.Errorf("sync parent directory: %w", err)
	}
	return nil
}
