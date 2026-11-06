package scripts_test

import (
	"fmt"
	"os"
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestInstallerBuildEnvironment(t *testing.T) {
	if _, err := exec.LookPath("sh"); err != nil {
		t.Skip("needs a POSIX shell")
	}
	source, err := os.ReadFile("../install.sh")
	if err != nil {
		t.Fatal(err)
	}
	var functions strings.Builder
	for _, name := range []string{"say", "note", "have", "go_usable", "prepare_build_env", "from_checkout", "from_go"} {
		functions.WriteString(installerFunction(t, string(source), name))
	}
	for _, method := range []string{"checkout", "go"} {
		for _, tc := range []struct {
			name, os, sdk, xcrun, cc, developer, goVersion string
			wantSDK, wantError                             string
			wantQuery                                      bool
		}{
			{name: "selected SDK", os: "Darwin", wantSDK: "selected", wantQuery: true},
			{name: "empty SDKROOT", os: "Darwin", sdk: "empty", wantSDK: "selected", wantQuery: true},
			{name: "explicit SDKROOT", os: "Darwin", sdk: "explicit", cc: "custom clang -target arm64-apple-macos", developer: "/custom developer", wantSDK: "explicit"},
			{name: "explicit compiler and developer", os: "Darwin", cc: "custom clang -target arm64-apple-macos", developer: "/custom developer", wantSDK: "selected", wantQuery: true},
			{name: "no xcrun with explicit SDKROOT", os: "Darwin", sdk: "explicit", xcrun: "absent", wantSDK: "explicit"},
			{name: "Linux without xcrun", os: "Linux", xcrun: "absent", wantSDK: "<unset>"},
			{name: "other OS", os: "FreeBSD", wantSDK: "<unset>"},
			{name: "Linux explicit environment", os: "Linux", sdk: "explicit", cc: "custom cc", developer: "/custom developer", wantSDK: "explicit"},
			{name: "missing xcrun", os: "Darwin", xcrun: "absent", wantSDK: "<unset>", wantError: "Cannot find the macOS SDK"},
			{name: "xcrun failure", os: "Darwin", xcrun: "failure", wantSDK: "<unset>", wantQuery: true, wantError: "Cannot find the macOS SDK"},
			{name: "empty xcrun result", os: "Darwin", xcrun: "empty", wantSDK: "<unset>", wantQuery: true, wantError: "Cannot find the macOS SDK"},
			{name: "missing SDK directory", os: "Darwin", xcrun: "missing", wantSDK: "<unset>", wantQuery: true, wantError: "Cannot find the macOS SDK"},
			{name: "old Go skips SDK discovery", os: "Darwin", goVersion: "go1.20.14", wantSDK: "<unset>", wantError: "needs Go 1.21 or newer"},
		} {
			t.Run(method+"/"+tc.name, func(t *testing.T) {
				root := t.TempDir()
				bin, checkout, temporary := filepath.Join(root, "bin"), filepath.Join(root, "checkout"), filepath.Join(root, "built")
				selected := filepath.Join(root, "selected SDK with spaces")
				for _, dir := range []string{bin, checkout, temporary, selected} {
					if err := os.MkdirAll(dir, 0o755); err != nil {
						t.Fatal(err)
					}
				}
				writeInstallerStub(t, bin, "uname", "printf '%s\\n' \"$TEST_OS\"\n")
				writeInstallerStub(t, bin, "go", `if [ "$1" = env ] && [ "$2" = GOVERSION ]; then
    printf '%s\n' "$TEST_GO_VERSION"
    exit 0
fi
printf 'go:%s|SDKROOT=%s|CC=%s|DEVELOPER_DIR=%s\n' "$*" "${SDKROOT-<unset>}" "${CC-<unset>}" "${DEVELOPER_DIR-<unset>}" >> "$TEST_LOG"
if [ "$1" = install ]; then
    name=${2##*/}
    : > "$GOBIN/${name%@*}"
fi
`)
				if tc.xcrun != "absent" {
					writeInstallerStub(t, bin, "xcrun", `printf 'xcrun:%s|DEVELOPER_DIR=%s\n' "$*" "${DEVELOPER_DIR-<unset>}" >> "$TEST_LOG"
[ "$TEST_XCRUN" != failure ] || exit 42
printf '%s\n' "$TEST_SDK"
`)
				}
				logPath := filepath.Join(root, "commands.log")
				if err := os.WriteFile(logPath, nil, 0o600); err != nil {
					t.Fatal(err)
				}
				env := map[string]string{
					"PATH": bin, "TEST_OS": tc.os, "TEST_LOG": logPath,
					"TEST_XCRUN": tc.xcrun, "TEST_SDK": selected,
					"TEST_CHECKOUT": checkout, "TEST_TEMPORARY": temporary,
					"TEST_GO_VERSION": "go1.27.0",
				}
				if tc.goVersion != "" {
					env["TEST_GO_VERSION"] = tc.goVersion
				}
				switch tc.xcrun {
				case "empty":
					env["TEST_SDK"] = ""
				case "missing":
					env["TEST_SDK"] = filepath.Join(root, "missing SDK")
				}
				switch tc.sdk {
				case "empty":
					env["SDKROOT"] = ""
				case "explicit":
					// An explicit SDKROOT is left alone, even if it is not a local directory.
					env["SDKROOT"] = "/explicit SDK"
				}
				if tc.cc != "" {
					env["CC"] = tc.cc
				}
				if tc.developer != "" {
					env["DEVELOPER_DIR"] = tc.developer
				}
				script := "set -eu\n" + functions.String() + `
ALL="kou-conveyor-runner kou-conveyor-tui kou-conveyor-web kou-conveyor-canvas"
MODULE=example.com/conveyor
version=latest
checkout=$TEST_CHECKOUT
tmp=$TEST_TEMPORARY
installed=
absent=
place() { installed="$installed $2"; }
` + "if from_" + method + `; then status=0; else status=$?; fi
printf 'state|SDKROOT=%s|CC=%s|DEVELOPER_DIR=%s\n' "${SDKROOT-<unset>}" "${CC-<unset>}" "${DEVELOPER_DIR-<unset>}"
exit "$status"
`
				command := exec.Command("sh", "-c", script)
				for _, entry := range os.Environ() {
					key, _, _ := strings.Cut(entry, "=")
					if _, replaced := env[key]; !replaced && key != "SDKROOT" && key != "CC" && key != "DEVELOPER_DIR" {
						command.Env = append(command.Env, entry)
					}
				}
				for key, value := range env {
					command.Env = append(command.Env, key+"="+value)
				}
				output, err := command.CombinedOutput()
				if (err != nil) != (tc.wantError != "") || !strings.Contains(string(output), tc.wantError) {
					t.Fatalf("result: %v, want error %q\n%s", err, tc.wantError, output)
				}
				wantSDK := tc.wantSDK
				switch wantSDK {
				case "selected":
					wantSDK = selected
				case "explicit":
					wantSDK = env["SDKROOT"]
				}
				cc, developer := "<unset>", "<unset>"
				if tc.cc != "" {
					cc = tc.cc
				}
				if tc.developer != "" {
					developer = tc.developer
				}
				wantEnv := fmt.Sprintf("|SDKROOT=%s|CC=%s|DEVELOPER_DIR=%s", wantSDK, cc, developer)
				if !strings.Contains(string(output), "state"+wantEnv+"\n") {
					t.Fatalf("environment: want %s\n%s", wantEnv, output)
				}
				logged, err := os.ReadFile(logPath)
				if err != nil {
					t.Fatal(err)
				}
				log := string(logged)
				queries := strings.Count(log, "xcrun:")
				if (queries == 1) != tc.wantQuery || queries > 1 {
					t.Fatalf("SDK queries = %d, want query %v\n%s", queries, tc.wantQuery, log)
				}
				if tc.wantQuery && !strings.Contains(log, "xcrun:--sdk macosx --show-sdk-path|DEVELOPER_DIR="+developer+"\n") {
					t.Fatalf("wrong SDK query\n%s", log)
				}
				builds := strings.Count(log, "go:")
				if tc.wantError != "" {
					if builds != 0 {
						t.Fatalf("built after preflight failure\n%s", log)
					}
					return
				}
				if builds != 4 || strings.Count(log, wantEnv+"\n") != 4 {
					t.Fatalf("want all four commands built with %s\n%s", wantEnv, log)
				}
				for _, name := range []string{"kou-conveyor-runner", "kou-conveyor-tui", "kou-conveyor-web", "kou-conveyor-canvas"} {
					packageArg := "./cmd/" + name
					if method == "go" {
						packageArg = "install example.com/conveyor/cmd/" + name + "@latest"
					}
					if !strings.Contains(log, packageArg+wantEnv+"\n") {
						t.Errorf("missing %s\n%s", packageArg, log)
					}
				}
			})
		}
	}
}

// Only definitions are evaluated; the installer's top level and place never run.
func installerFunction(t *testing.T, source, name string) string {
	t.Helper()
	start := strings.Index(source, "\n"+name+"() {")
	if start == -1 {
		t.Fatalf("missing installer function %s", name)
	}
	body := source[start+1:]
	line, _, _ := strings.Cut(body, "\n")
	if strings.HasSuffix(line, "}") {
		return line + "\n"
	}
	end := strings.Index(body, "\n}\n")
	if end == -1 {
		t.Fatalf("unterminated installer function %s", name)
	}
	return body[:end+3]
}

func writeInstallerStub(t *testing.T, bin, name, source string) {
	t.Helper()
	if err := os.WriteFile(filepath.Join(bin, name), []byte("#!/bin/sh\nset -eu\n"+source), 0o755); err != nil {
		t.Fatal(err)
	}
}
