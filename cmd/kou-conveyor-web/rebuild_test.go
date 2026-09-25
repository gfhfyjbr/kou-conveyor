package main

import (
	"context"
	"net"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"
)

// The fingerprint follows the programs' Go code, and nothing else.
func TestGoFingerprintFollowsTheCode(t *testing.T) {
	root := t.TempDir()
	writeFiles(t, root, map[string]string{
		"go.mod":                       "module example.com/x\n",
		"cmd/tool/main.go":             "package main\nfunc main() {}\n",
		"harness/lib/lib.go":           "package lib\n",
		"harness/lib/lib_test.go":      "package lib\n",
		"cmd/tool/plugins/x/web/x.js":  "export default () => {};",
		"cmd/tool/static/index.html":   "<p>",
		"internal/gen/testdata/x.go":   "package x\n",
		"cmd/tool/.hidden/secret.go":   "package h\n",
		"README.md":                    "readme",
		"benchmarks/harbor/runner.go":  "package harbor\n",
		"harness/lib/notes/readme.txt": "x",
	})
	before := goFingerprint(root)
	step := func(name string, change func(), changes bool) {
		t.Helper()
		last := goFingerprint(root)
		time.Sleep(5 * time.Millisecond)
		change()
		if now := goFingerprint(root); (now != last) != changes {
			t.Errorf("%s: changed %v, want %v", name, now != last, changes)
		}
	}
	later := time.Now().Add(time.Second)
	touch := func(path string) func() {
		return func() {
			full := filepath.Join(root, path)
			os.WriteFile(full, []byte("package x // changed\n"), 0o644)
			os.Chtimes(full, later, later)
		}
	}
	step("a test", touch("harness/lib/lib_test.go"), false)
	step("a plugin's file", touch("cmd/tool/plugins/x/web/x.js"), false)
	step("testdata", touch("internal/gen/testdata/x.go"), false)
	step("a hidden directory", touch("cmd/tool/.hidden/secret.go"), false)
	step("code outside the programs", touch("benchmarks/harbor/runner.go"), false)
	step("a program's code", touch("harness/lib/lib.go"), true)
	step("go.mod", touch("go.mod"), true)
	step("a new file", func() { writeFiles(t, root, map[string]string{"cmd/tool/more.go": "package main\n"}) }, true)
	step("a file removed", func() { os.Remove(filepath.Join(root, "cmd/tool/more.go")) }, true)
	if before == goFingerprint(root) {
		t.Error("the fingerprint did not change at all")
	}
}

// A build puts the new programs in place of those beside the server; one
// that fails leaves them as they were and says why.
func TestRebuildBuildsAndPlacesThePrograms(t *testing.T) {
	if testing.Short() {
		t.Skip("builds a program")
	}
	root, installed := t.TempDir(), t.TempDir()
	writeFiles(t, root, map[string]string{
		"go.mod":                          "module example.com/cockpit\n\ngo 1.21\n",
		"cmd/kou-conveyor-web/main.go":    "package main\n\nvar builtFrom, sourceFingerprint string\n\nfunc main() { println(builtFrom, sourceFingerprint) }\n",
		"cmd/kou-conveyor-runner/main.go": "package main\n\nfunc main() {}\n",
	})
	writeFiles(t, installed, map[string]string{"kou-conveyor-web": "old", "kou-conveyor-runner": "old"})
	h := newHarness(t)
	h.server.rebuild = &rebuilder{root: root, dir: installed, exe: filepath.Join(installed, "kou-conveyor-web")}
	stream := h.pluginEvents("/api/plugins/events")
	stream.next("plugins")

	fingerprint := goFingerprint(root)
	if err := h.server.build(context.Background(), fingerprint); err != nil {
		t.Fatalf("build: %v, %+v", err, h.server.rebuild.current())
	}
	if building := stream.next("server"); building["state"] != "building" {
		t.Fatalf("status = %v", building)
	}
	for _, name := range []string{"kou-conveyor-web", "kou-conveyor-runner"} {
		data, err := os.ReadFile(filepath.Join(installed, name))
		if err != nil || string(data) == "old" || len(data) < 1000 {
			t.Fatalf("%s was not placed: %d bytes, %v", name, len(data), err)
		}
	}
	if _, err := os.Stat(filepath.Join(installed, "kou-conveyor-tui")); err == nil {
		t.Fatal("a program not installed was built")
	}

	// A build that fails.
	placed, _ := os.ReadFile(filepath.Join(installed, "kou-conveyor-web"))
	writeFiles(t, root, map[string]string{"cmd/kou-conveyor-web/main.go": "package main\n\nfunc main() { undefinedThing() }\n"})
	if err := h.server.build(context.Background(), goFingerprint(root)); err == nil {
		t.Fatal("a broken build succeeded")
	}
	status := h.server.rebuild.current()
	if status.State != "failed" || !strings.Contains(status.Output, "undefinedThing") {
		t.Fatalf("status = %+v", status)
	}
	if failed := stream.until2("server", func(event map[string]any) bool { return event["state"] == "failed" }); !strings.Contains(failed["output"].(string), "undefinedThing") {
		t.Fatalf("the pages were told %v", failed)
	}
	if still, _ := os.ReadFile(filepath.Join(installed, "kou-conveyor-web")); string(still) != string(placed) {
		t.Fatal("a failed build replaced the program")
	}
}

// until2 waits for an event of a kind that satisfies ok.
func (stream *pluginEvents) until2(kind string, ok func(map[string]any) bool) map[string]any {
	stream.t.Helper()
	for {
		if event := stream.next(kind); ok(event) {
			return event
		}
	}
}

// A server that restarts in place takes up the socket the build before it
// handed over.
func TestInheritedListener(t *testing.T) {
	if listener, err := inheritedListener(); listener != nil || err != nil {
		t.Fatalf("without one: %v, %v", listener, err)
	}
	original, err := net.Listen("tcp", "127.0.0.1:0")
	if err != nil {
		t.Fatal(err)
	}
	defer original.Close()
	file, err := original.(*net.TCPListener).File()
	if err != nil {
		t.Fatal(err)
	}
	defer file.Close()
	t.Setenv(listenerEnvironment, strings.TrimSpace(func() string { return itoa(int(file.Fd())) }()))
	listener, err := inheritedListener()
	if err != nil || listener == nil {
		t.Fatalf("inherited: %v, %v", listener, err)
	}
	defer listener.Close()
	if listener.Addr().String() != original.Addr().String() {
		t.Fatalf("address = %s, want %s", listener.Addr(), original.Addr())
	}
	if os.Getenv(listenerEnvironment) != "" {
		t.Fatal("the socket's variable outlived its use")
	}
	go func() {
		if conn, err := net.Dial("tcp", listener.Addr().String()); err == nil {
			conn.Close()
		}
	}()
	listener.(*net.TCPListener).SetDeadline(time.Now().Add(5 * time.Second))
	conn, err := listener.Accept()
	if err != nil {
		t.Fatalf("accept on the inherited socket: %v", err)
	}
	conn.Close()
}

func itoa(n int) string {
	digits := ""
	for {
		digits = string(rune('0'+n%10)) + digits
		n /= 10
		if n == 0 {
			return digits
		}
	}
}
