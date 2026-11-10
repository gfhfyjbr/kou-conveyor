// Command kou-codevm-oracle compares controlled Code VM semantics during migration.
// Tests register fake plugin tools. Native built-ins remain, so the corpus must
// be trusted. This command never calls a provider.
package main

import (
	"context"
	"encoding/json/v2"
	"fmt"
	"io"
	"os"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/codevm"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run() error {
	if len(os.Args) == 3 && os.Args[1] == "fake-tool" {
		if os.Args[2] == "fail" {
			fmt.Print("host failed")
			os.Exit(1)
		}
		data, err := io.ReadAll(io.LimitReader(os.Stdin, 4<<20))
		if err != nil {
			return err
		}
		_, err = fmt.Fprintf(os.Stdout, "echo:%s", data)
		return err
	}
	if len(os.Args) != 1 {
		return fmt.Errorf("unexpected arguments")
	}
	var input struct {
		Code string `json:"code"`
	}
	data, err := io.ReadAll(io.LimitReader(os.Stdin, 1<<20))
	if err != nil {
		return err
	}
	if err := json.Unmarshal(data, &input); err != nil {
		return err
	}
	// The corpus is trusted test input. The Go VM has real built-ins, so this
	// oracle must not be used as an untrusted-code sandbox.
	directory, err := os.MkdirTemp("", "kou-codevm-oracle-")
	if err != nil {
		return err
	}
	defer os.RemoveAll(directory)
	executable, err := os.Executable()
	if err != nil {
		return err
	}
	tools := []codevm.CommandTool{}
	for _, name := range []string{"echo", "net.fetch", "JSON", "bash", "fail"} {
		tools = append(tools, codevm.CommandTool{Name: name, Command: []string{executable, "fake-tool", name}})
	}
	result := codevm.Run(context.Background(), codevm.Config{Directory: directory, Shell: "/bin/sh", Tools: tools, RunTimeout: 1, CommandTimeout: 1, MaxCalls: 3}, input.Code, nil)
	// Stack frames contain engine-specific source locations. Their first line
	// carries the observable exception class/message used by this corpus.
	result.Error = strings.SplitN(result.Error, "\n", 2)[0]
	encoded, err := json.Marshal(result)
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(append(encoded, '\n'))
	return err
}
