package main

import (
	"testing"
)

func TestCaptureProjectionUsesOwnedRealProcesses(t *testing.T) {
	result, err := run(spec{Code: "test -f /dev/stdout && test -f /dev/stderr || exit 91; printf out; printf err >&2; exit 7"})
	if err != nil {
		t.Fatal(err)
	}
	if !result.Started || !result.Exited || result.Canceled || result.ExitCode == nil || *result.ExitCode != 7 || result.Signal == nil || *result.Signal != 0 || result.Error != "" {
		t.Fatalf("lifecycle = %#v", result)
	}
	if result.StdoutHex != "6f7574" || result.StderrHex != "657272" || !result.GroupGone || result.OutMode != 0o600 || result.ErrMode != 0o600 {
		t.Fatalf("captures = %#v", result)
	}
}

func TestCanceledCaptureProjectionPreservesUnstartedFiles(t *testing.T) {
	result, err := run(spec{Code: "printf must-not-run", CancelBefore: true})
	if err != nil {
		t.Fatal(err)
	}
	if result.Started || !result.Canceled || result.Exited || result.Error != "" || result.StdoutHex != "6f6c642063617074757265" || result.StderrHex != result.StdoutHex || !result.GroupGone {
		t.Fatalf("canceled = %#v", result)
	}
}

func TestCaptureInputAndCancellationAreJoined(t *testing.T) {
	input := "owned stdin"
	result, err := run(spec{Code: "/bin/cat", Input: &input})
	if err != nil {
		t.Fatal(err)
	}
	if result.StdoutHex != "6f776e656420737464696e" || result.ExitCode == nil || *result.ExitCode != 0 || !result.GroupGone {
		t.Fatalf("input = %#v", result)
	}
	result, err = run(spec{Code: "trap 'printf final; exit 0' TERM; printf ready; while :; do :; done", CancelOnReady: true})
	if err != nil {
		t.Fatal(err)
	}
	if !result.Canceled || !result.Started || result.Exited || result.StdoutHex != "726561647966696e616c" || !result.GroupGone {
		t.Fatalf("cancel = %#v", result)
	}
}
