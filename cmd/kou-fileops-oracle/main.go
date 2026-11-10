// Command kou-fileops-oracle exercises file tools on trusted temporary fixtures.
// It changes files named by its input; it is not an untrusted-code sandbox.
package main

import (
	// v1 provides a rune-replacement projection for invalid UTF-8 display.
	// Source file bytes are compared independently by the fixture driver.
	"encoding/json"
	"fmt"
	"io"
	"os"

	"github.com/gfhfyjbr/kou-conveyor/harness/fileops"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run() error {
	var request struct {
		Action, Name, Content, Old, Replacement string
		ReadOptions                             fileops.ReadOptions
		EditOptions                             fileops.EditOptions
	}
	data, err := io.ReadAll(io.LimitReader(os.Stdin, 8<<20))
	if err != nil {
		return err
	}
	if err := json.Unmarshal(data, &request); err != nil {
		return err
	}
	var value any
	switch request.Action {
	case "read":
		value, err = fileops.Read(request.Name, request.ReadOptions)
	case "write":
		value, err = fileops.Write(request.Name, request.Content)
	case "edit":
		value, err = fileops.Edit(request.Name, request.Old, request.Replacement, request.EditOptions)
	case "patch":
		var result fileops.PatchResult
		result, err = fileops.ApplyPatch(request.Content, request.Name)
		value = struct {
			Summary string
			Files   []string
		}{result.Summary(), result.Files()}
	default:
		return fmt.Errorf("unsupported fixture action %q", request.Action)
	}
	message := ""
	if err != nil {
		// The partial ReadResult returned with an offset error is not public
		// tool output. Compare the error, not that internal partial result.
		message, value = err.Error(), nil
	}
	encoded, err := json.Marshal(struct {
		Result any
		Error  string
	}{value, message})
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(append(encoded, '\n'))
	return err
}
