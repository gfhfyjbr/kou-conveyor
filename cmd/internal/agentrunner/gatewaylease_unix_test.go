//go:build unix

package agentrunner

import (
	"bytes"
	"context"
	"os"
	"os/exec"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"golang.org/x/sys/unix"
)

func TestGatewayLeaseIsCloseOnExecBeforeProviderAndTools(t *testing.T) {
	if os.Getenv("KOU_TEST_GATEWAY_LEASE_FD") == "1" {
		client := &fakeClient{}
		client.respond = func(context.Context, llm.Request) (llm.Response, error) {
			for _, fd := range []uintptr{3, 4} {
				flags, err := unix.FcntlInt(fd, unix.F_GETFD, 0)
				if err != nil || flags&unix.FD_CLOEXEC == 0 {
					t.Fatalf("inherited descriptor %d can leak to a command: flags=%d err=%v", fd, flags, err)
				}
			}
			return llm.Response{ID: "response-1", Stop: llm.StopComplete, Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: "done"}}}}, nil
		}
		var stdout, stderr bytes.Buffer
		code := RunMain(t.Context(), []string{"-workspace", t.TempDir(), "-session-directory", t.TempDir(), "-detached", "-gateway-lease"},
			steerEnv, func() []string { return nil }, strings.NewReader(`{"session_id":"lease","messages":[{"content":"hello"}],"model":"gpt-test"}`), &stdout, &stderr, testConfig(client))
		if code != 0 {
			t.Fatalf("runner: %d %s", code, stderr.String())
		}
		return
	}
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	lock, err := os.CreateTemp(t.TempDir(), "lock")
	if err != nil {
		t.Fatal(err)
	}
	defer lock.Close()
	read, write, err := os.Pipe()
	if err != nil {
		t.Fatal(err)
	}
	defer read.Close()
	defer write.Close()
	cmd := exec.CommandContext(t.Context(), exe, "-test.run=^TestGatewayLeaseIsCloseOnExecBeforeProviderAndTools$")
	cmd.Env = append(os.Environ(), "KOU_TEST_GATEWAY_LEASE_FD=1")
	cmd.ExtraFiles = []*os.File{lock, write}
	if output, err := cmd.CombinedOutput(); err != nil {
		t.Fatalf("runner descriptor contract: %v\n%s", err, output)
	}
}
