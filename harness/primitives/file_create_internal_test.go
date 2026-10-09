//go:build unix

package primitives

import (
	"context"
	"errors"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestCreateNewPathUsesCurrentDirectory(t *testing.T) {
	t.Chdir(t.TempDir())

	result, err := createNewPath(t.Context(), IOCreateRequest{
		Kind: IOCreateRegularFile,
		Path: "created",
		Mode: IOCreateDefaultMode,
	})
	if err != nil {
		t.Fatal(err)
	}
	if result.Kind != IOCreateRegularFile {
		t.Fatalf("kind = %d, want regular file", result.Kind)
	}
	if _, err := os.Stat("created"); err != nil {
		t.Fatal(err)
	}
}

func TestCreateNewPathUsesRequestedMode(t *testing.T) {
	mode := os.FileMode(0o600)
	path := filepath.Join(t.TempDir(), "created")

	_, err := createNewPath(t.Context(), IOCreateRequest{Kind: IOCreateRegularFile, Path: path, Mode: mode})
	if err != nil {
		t.Fatal(err)
	}
	info, err := os.Stat(path)
	if err != nil {
		t.Fatal(err)
	}
	if info.Mode().Perm() != mode {
		t.Fatalf("mode = %o, want %o", info.Mode().Perm(), mode)
	}
}

func TestUseExistingPathRequestsRetryForMissingEntry(t *testing.T) {
	parentPath := t.TempDir()
	parent, err := openCreateDirectory(parentPath)
	if err != nil {
		t.Fatal(err)
	}
	defer parent.Close()
	request := IOCreateRequest{
		Kind: IOCreateRegularFile,
		Path: filepath.Join(parentPath, "missing"),
		Mode: 0o600,
	}
	result, retry, err := useExistingPath(request, "missing", parentPath, parent)
	if err != nil || !retry || result != (IOCreateResult{}) {
		t.Fatalf("use missing path = (%#v, %t, %v), want empty result and retry", result, retry, err)
	}
	if _, err := parent.Stat(); err != nil {
		t.Fatalf("parent must remain open for retry: %v", err)
	}
	if _, err := os.Stat(request.Path); !errors.Is(err, os.ErrNotExist) {
		t.Fatalf("retry request created an entry: %v", err)
	}
}

func TestCreateOrUsePathHonorsCancellationBeforeMutation(t *testing.T) {
	parentPath := t.TempDir()
	parent, err := openCreateDirectory(parentPath)
	if err != nil {
		t.Fatal(err)
	}
	request := IOCreateRequest{
		Kind: IOCreateRegularFile,
		Path: filepath.Join(parentPath, "created"),
		Mode: 0o600,
	}
	mode, err := unixFileMode(request.Mode)
	if err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	_, err = createOrUsePath(ctx, request, "created", parentPath, parent, mode)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("error = %v, want context cancellation", err)
	}
	if _, err := os.Stat(request.Path); !os.IsNotExist(err) {
		t.Fatalf("stat canceled path: %v, want not exist", err)
	}
}

func TestCreateOrUsePathPreservesCloseFailureDuringCancellation(t *testing.T) {
	parentPath := t.TempDir()
	parent, err := openCreateDirectory(parentPath)
	if err != nil {
		t.Fatal(err)
	}
	if err := parent.Close(); err != nil {
		t.Fatal(err)
	}
	ctx, cancel := context.WithCancel(t.Context())
	cancel()

	_, err = createOrUsePath(ctx, IOCreateRequest{}, "created", parentPath, parent, 0o600)
	if !errors.Is(err, context.Canceled) || isOnlyContextCancellation(err) {
		t.Fatalf("error = %v, want cancellation joined with close failure", err)
	}
}

func TestUnixFileMode(t *testing.T) {
	mode := os.FileMode(0o640) | os.ModeSetuid | os.ModeSetgid | os.ModeSticky
	got, err := unixFileMode(mode)
	if err != nil {
		t.Fatal(err)
	}
	want := uint32(0o640 | 0o4000 | 0o2000 | 0o1000)
	if got != want {
		t.Fatalf("mode = %#o, want %#o", got, want)
	}
}

func TestCreateNewPathRejectsNonUnixModeBits(t *testing.T) {
	path := filepath.Join(t.TempDir(), "created")
	_, err := createNewPath(t.Context(), IOCreateRequest{
		Kind: IOCreateRegularFile,
		Path: path,
		Mode: os.ModeDir | 0o700,
	})
	if err == nil || !strings.Contains(err.Error(), "unsupported mode bits") {
		t.Fatalf("error = %v, want unsupported mode failure", err)
	}
	if _, statErr := os.Stat(path); !os.IsNotExist(statErr) {
		t.Fatalf("stat rejected path: %v, want not exist", statErr)
	}
}

func TestOpenCreateDirectoryRejectsFile(t *testing.T) {
	path := filepath.Join(t.TempDir(), "file")
	file, err := os.Create(path)
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}

	directory, err := openCreateDirectory(path)
	if err == nil {
		if closeErr := directory.Close(); closeErr != nil {
			t.Error(closeErr)
		}
		t.Fatal("opening a file as the parent directory succeeded")
	}
}

func TestOpenNewFileAtRejectsClosedDirectory(t *testing.T) {
	parent, err := openCreateDirectory(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if err := parent.Close(); err != nil {
		t.Fatal(err)
	}

	mode, err := unixFileMode(IOCreateDefaultMode)
	if err != nil {
		t.Fatal(err)
	}
	file, err := openNewFileAt(parent, "created", mode)
	if err == nil {
		if closeErr := file.Close(); closeErr != nil {
			t.Error(closeErr)
		}
		t.Fatal("creating relative to a closed directory succeeded")
	}
}

func TestMakeNewDirectoryAtRejectsClosedDirectory(t *testing.T) {
	parentPath := t.TempDir()
	parent, err := openCreateDirectory(parentPath)
	if err != nil {
		t.Fatal(err)
	}
	if err := parent.Close(); err != nil {
		t.Fatal(err)
	}

	err = makeNewDirectoryAt(parent, "created", 0o700)
	if err == nil {
		t.Fatal("creating relative to a closed directory succeeded")
	}
	if _, statErr := os.Stat(filepath.Join(parentPath, "created")); !os.IsNotExist(statErr) {
		t.Fatalf("stat rejected directory: %v, want not exist", statErr)
	}
}

func TestSyncCreatedFileReportsFileSyncFailure(t *testing.T) {
	parent, err := openCreateDirectory(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := parent.Close(); err != nil {
			t.Error(err)
		}
	})

	file, err := os.CreateTemp(t.TempDir(), "created-")
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}

	err = syncCreatedFile(file, parent)
	if err == nil || !strings.Contains(err.Error(), "sync file") {
		t.Fatalf("error = %v, want file synchronization failure", err)
	}
}

func TestSyncCreatedFileReportsParentSyncFailure(t *testing.T) {
	file, err := os.CreateTemp(t.TempDir(), "created-")
	if err != nil {
		t.Fatal(err)
	}
	t.Cleanup(func() {
		if err := file.Close(); err != nil {
			t.Error(err)
		}
	})

	parent, err := openCreateDirectory(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if err := parent.Close(); err != nil {
		t.Fatal(err)
	}

	err = syncCreatedFile(file, parent)
	if err == nil || !strings.Contains(err.Error(), "sync parent directory") {
		t.Fatalf("error = %v, want parent-directory synchronization failure", err)
	}
}

func TestSyncCreatedDirectoryReportsParentSyncFailure(t *testing.T) {
	parent, err := openCreateDirectory(t.TempDir())
	if err != nil {
		t.Fatal(err)
	}
	if err := parent.Close(); err != nil {
		t.Fatal(err)
	}

	err = syncCreatedDirectory(parent)
	if err == nil || !strings.Contains(err.Error(), "sync parent directory") {
		t.Fatalf("error = %v, want parent-directory synchronization failure", err)
	}
}

func TestPersistCreatedFileReportsSyncAndCloseFailures(t *testing.T) {
	parentPath := t.TempDir()
	parent, err := openCreateDirectory(parentPath)
	if err != nil {
		t.Fatal(err)
	}

	file, err := os.CreateTemp(parentPath, "created-")
	if err != nil {
		t.Fatal(err)
	}
	path := file.Name()
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}

	_, err = persistCreatedFile(IOCreateRequest{
		Kind: IOCreateRegularFile,
		Path: path,
	}, parentPath, file, parent)
	if err == nil {
		t.Fatal("persisting a closed file succeeded")
	}
	for _, want := range []string{"sync file", "close file"} {
		if !strings.Contains(err.Error(), want) {
			t.Fatalf("error = %q, want %q", err, want)
		}
	}
}

func TestCloseFileReportsFailure(t *testing.T) {
	file, err := os.CreateTemp(t.TempDir(), "created-")
	if err != nil {
		t.Fatal(err)
	}
	if err := file.Close(); err != nil {
		t.Fatal(err)
	}

	err = closeFile("file", file.Name(), file)
	if err == nil || !errors.Is(err, os.ErrClosed) {
		t.Fatalf("error = %v, want closed-file error", err)
	}
}
