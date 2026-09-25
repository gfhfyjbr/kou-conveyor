package primitives

import (
	"errors"
	"fmt"
	"os"

	"golang.org/x/sys/unix"
)

func syncCreatedFile(file *os.File, parent *os.File) error {
	if err := fsync(file); err != nil {
		return fmt.Errorf("sync file: %w", err)
	}
	return syncCreatedDirectory(parent)
}

func syncCreatedDirectory(parent *os.File) error {
	if err := fsync(parent); err != nil {
		return fmt.Errorf("sync parent directory: %w", err)
	}
	err := retryEINTR(func() error {
		_, err := unix.FcntlInt(parent.Fd(), unix.F_FULLFSYNC, 0)
		return err
	})
	return fullSyncResult(err)
}

func fullSyncResult(err error) error {
	if errors.Is(err, unix.ENOTSUP) {
		return nil
	}
	if err != nil {
		return fmt.Errorf("flush storage device: %w", err)
	}
	return nil
}

func fsync(file *os.File) error {
	return retryEINTR(func() error {
		return unix.Fsync(int(file.Fd()))
	})
}
