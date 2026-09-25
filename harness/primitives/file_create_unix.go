//go:build unix

package primitives

import (
	"context"
	"errors"
	"fmt"
	"os"
	"path/filepath"

	"golang.org/x/sys/unix"
)

const maxCreateContentionRetries = 16

func createNewPath(ctx context.Context, request IOCreateRequest) (IOCreateResult, error) {
	if request.Kind != IOCreateRegularFile && request.Kind != IOCreateDirectory {
		return IOCreateResult{}, fmt.Errorf("create %q: unsupported kind %d", request.Path, request.Kind)
	}
	mode, err := unixFileMode(request.Mode)
	if err != nil {
		return IOCreateResult{}, fmt.Errorf("create %q: %w", request.Path, err)
	}

	path := request.Path
	if request.Kind == IOCreateDirectory {
		path = trimTrailingPathSeparators(path)
	}
	parentPath, name := filepath.Split(path)
	if parentPath == "" {
		parentPath = "."
	}

	parent, err := openCreateDirectory(parentPath)
	if err != nil {
		return IOCreateResult{}, fmt.Errorf("open parent directory %q: %w", parentPath, err)
	}
	return createOrUsePath(ctx, request, name, parentPath, parent, mode)
}

func createOrUsePath(
	ctx context.Context,
	request IOCreateRequest,
	name string,
	parentPath string,
	parent *os.File,
	mode uint32,
) (IOCreateResult, error) {
	for retries := 0; ; {
		if err := ctx.Err(); err != nil {
			return IOCreateResult{}, errors.Join(
				err,
				closeFile("parent directory", parentPath, parent),
			)
		}
		if request.Kind == IOCreateRegularFile {
			file, err := openNewFileAt(parent, name, mode)
			if err == nil {
				return persistCreatedFile(request, parentPath, file, parent)
			}
			if !errors.Is(err, os.ErrExist) {
				return IOCreateResult{}, errors.Join(
					fmt.Errorf("create %q: %w", request.Path, err),
					closeFile("parent directory", parentPath, parent),
				)
			}
		} else {
			err := makeNewDirectoryAt(parent, name, mode)
			if err == nil {
				return persistCreatedEntry(request, parentPath, parent)
			}
			if !errors.Is(err, os.ErrExist) {
				return IOCreateResult{}, errors.Join(
					fmt.Errorf("create %q: %w", request.Path, err),
					closeFile("parent directory", parentPath, parent),
				)
			}
		}

		result, retry, err := useExistingPath(request, name, parentPath, parent)
		if !retry {
			return result, err
		}
		if retries == maxCreateContentionRetries {
			return IOCreateResult{}, errors.Join(
				fmt.Errorf(
					"create %q: path remained unstable after %d retries",
					request.Path,
					maxCreateContentionRetries,
				),
				closeFile("parent directory", parentPath, parent),
			)
		}
		retries++
	}
}

func useExistingPath(
	request IOCreateRequest,
	name string,
	parentPath string,
	parent *os.File,
) (IOCreateResult, bool, error) {
	var stat unix.Stat_t
	if err := retryEINTR(func() error {
		return unix.Fstatat(int(parent.Fd()), name, &stat, unix.AT_SYMLINK_NOFOLLOW)
	}); err != nil {
		if errors.Is(err, os.ErrNotExist) {
			return IOCreateResult{}, true, nil
		}
		return IOCreateResult{}, false, errors.Join(
			fmt.Errorf("inspect existing %q: %w", request.Path, err),
			closeFile("parent directory", parentPath, parent),
		)
	}
	if err := validateExistingPath(request, uint32(stat.Mode)); err != nil {
		return IOCreateResult{}, false, errors.Join(
			fmt.Errorf("use existing %q: %w", request.Path, err),
			closeFile("parent directory", parentPath, parent),
		)
	}
	result, err := persistCreatedEntry(request, parentPath, parent)
	return result, false, err
}

func validateExistingPath(request IOCreateRequest, mode uint32) error {
	if request.Kind == IOCreateRegularFile && mode&unix.S_IFMT != unix.S_IFREG {
		return errors.New("existing path is not a regular file")
	}
	if request.Kind == IOCreateDirectory && mode&unix.S_IFMT != unix.S_IFDIR {
		return errors.New("existing path is not a directory")
	}
	return nil
}

func trimTrailingPathSeparators(path string) string {
	end := len(path)
	for end > 0 && os.IsPathSeparator(path[end-1]) {
		end--
	}
	if end == 0 {
		return path
	}
	return path[:end]
}

func persistCreatedFile(
	request IOCreateRequest,
	parentPath string,
	file *os.File,
	parent *os.File,
) (IOCreateResult, error) {
	if err := syncCreatedFile(file, parent); err != nil {
		return IOCreateResult{}, errors.Join(
			fmt.Errorf("persist %q: %w", request.Path, err),
			closeFile("file", request.Path, file),
			closeFile("parent directory", parentPath, parent),
		)
	}

	// The creation is already durable; descriptor cleanup cannot undo the commit.
	_ = file.Close()
	_ = parent.Close()
	return IOCreateResult{Kind: request.Kind}, nil
}

func persistCreatedEntry(
	request IOCreateRequest,
	parentPath string,
	parent *os.File,
) (IOCreateResult, error) {
	if err := syncCreatedDirectory(parent); err != nil {
		return IOCreateResult{}, errors.Join(
			fmt.Errorf("persist %q: %w", request.Path, err),
			closeFile("parent directory", parentPath, parent),
		)
	}

	// The creation is already durable; descriptor cleanup cannot undo the commit.
	_ = parent.Close()
	return IOCreateResult{Kind: request.Kind}, nil
}

func openCreateDirectory(path string) (*os.File, error) {
	return os.OpenFile(path, os.O_RDONLY|unix.O_DIRECTORY, 0)
}

func openNewFileAt(parent *os.File, name string, mode uint32) (*os.File, error) {
	var fd int
	err := retryEINTR(func() error {
		var openErr error
		fd, openErr = unix.Openat(
			int(parent.Fd()),
			name,
			unix.O_WRONLY|unix.O_CREAT|unix.O_EXCL|unix.O_CLOEXEC,
			mode,
		)
		return openErr
	})
	if err != nil {
		return nil, err
	}
	return os.NewFile(uintptr(fd), name), nil
}

func makeNewDirectoryAt(parent *os.File, name string, mode uint32) error {
	return retryEINTR(func() error {
		return unix.Mkdirat(int(parent.Fd()), name, mode)
	})
}

func unixFileMode(mode os.FileMode) (uint32, error) {
	const supported = os.ModePerm | os.ModeSetuid | os.ModeSetgid | os.ModeSticky
	if unsupported := mode &^ supported; unsupported != 0 {
		return 0, fmt.Errorf("unsupported mode bits %v", unsupported)
	}

	result := uint32(mode.Perm())
	if mode&os.ModeSetuid != 0 {
		result |= unix.S_ISUID
	}
	if mode&os.ModeSetgid != 0 {
		result |= unix.S_ISGID
	}
	if mode&os.ModeSticky != 0 {
		result |= unix.S_ISVTX
	}
	return result, nil
}

func closeFile(kind string, path string, file *os.File) error {
	if err := file.Close(); err != nil {
		return fmt.Errorf("close %s %q: %w", kind, path, err)
	}
	return nil
}
