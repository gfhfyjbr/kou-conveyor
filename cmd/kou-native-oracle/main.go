// Command kou-native-oracle runs only the trusted temporary-fixture corpus.
// Unlike the fake-tools VM oracle, it performs real file and process operations.
package main

import (
	"context"
	"encoding/json/v2"
	"fmt"
	"io"
	"os"
	"path/filepath"

	"github.com/gfhfyjbr/kou-conveyor/harness/codevm"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func run() error {
	var request struct{ Code, Directory string }
	data, err := io.ReadAll(io.LimitReader(os.Stdin, 1<<20))
	if err != nil {
		return err
	}
	if err := json.Unmarshal(data, &request); err != nil {
		return err
	}
	if !filepath.IsAbs(request.Directory) {
		return fmt.Errorf("fixture directory must be absolute")
	}
	tools := []codevm.CommandTool{
		{Name: "echo", Command: []string{"/bin/cat"}},
		{Name: "probe", Command: []string{"/bin/sh", "-c", `printf '%s:%s:%s:' "$KOU_CONVEYOR_TOOL_NAME" "$KOU_CONVEYOR_TOOL_CALL_ID" "$EXTRA"; cat`}, Environment: map[string]string{"EXTRA": "own"}},
		{Name: "fail", Command: []string{"/bin/sh", "-c", "printf reason; exit 5"}},
	}
	config := codevm.Config{Shell: "/bin/sh", Directory: request.Directory, Environment: []string{}, RunTimeout: 3, CommandTimeout: 1, MaxCalls: 30, CallID: "native", Tools: tools, Skills: map[string]string{"example": filepath.Join(request.Directory, "SKILL.md")}}
	result := codevm.Run(context.Background(), config, request.Code, nil)
	encoded, err := json.Marshal(result)
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(append(encoded, '\n'))
	return err
}
