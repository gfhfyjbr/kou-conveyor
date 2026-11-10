// Command kou-conveyor-oracle exposes the Go store to migration tests. It is
// development tooling, not part of the Rust application's runtime.
package main

import (
	"context"
	"encoding/json/v2"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
)

type archive struct {
	Snapshot         sessionstore.Snapshot
	Items            []sessionstore.Item
	Operations       []operation.Operation
	ExternalInputIDs []inbox.ID
}

func main() {
	file := flag.String("session", "", "existing format-2 session file")
	action := flag.String("action", "read", "read (default), resume, append, checkpoint, hold, or try-lock")
	flag.Parse()
	if *file == "" || flag.NArg() != 0 {
		fmt.Fprintln(os.Stderr, "usage: kou-conveyor-oracle -session FILE.session.jsonl [-action read|resume|append|checkpoint|hold|try-lock]")
		os.Exit(2)
	}
	if err := run(*file, *action); err != nil {
		fmt.Fprintln(os.Stderr, "oracle:", err)
		os.Exit(1)
	}
}

func run(path, action string) error {
	id, ok := strings.CutSuffix(filepath.Base(path), ".session.jsonl")
	if !ok {
		return fmt.Errorf("expected .session.jsonl suffix")
	}
	if action != "read" && action != "resume" && action != "checkpoint" && action != "append" && action != "hold" && action != "try-lock" {
		return fmt.Errorf("unknown action")
	}
	// Refuse to create a missing directory through localfile.New.
	info, err := os.Stat(filepath.Dir(path))
	if err != nil {
		return err
	}
	if !info.IsDir() {
		return fmt.Errorf("session directory is not a directory")
	}
	if action != "read" && action != "resume" {
		release, err := cockpit.LockSession(filepath.Dir(path), id)
		if err != nil {
			return err
		}
		defer release()
		if action == "hold" || action == "try-lock" {
			fmt.Fprintln(os.Stdout, "locked")
			if action == "hold" {
				_, err = io.Copy(io.Discard, os.Stdin)
			}
			return err
		}
	}
	store, err := localfile.New(filepath.Dir(path))
	if err != nil {
		return err
	}
	ctx := context.Background()
	if action == "append" {
		if err := store.AppendInput(ctx, session.ID(id), inbox.Input{
			ID: "go-continued", Kind: inbox.InputExternal, Payload: []byte(`{"message":"continued by Go"}`),
		}); err != nil {
			return err
		}
	}
	if action == "checkpoint" {
		if err := store.SaveCheckpoint(ctx, session.ID(id), []byte(`{"generation":2}`)); err != nil {
			return err
		}
	}
	if action == "resume" {
		value, err := store.Resume(ctx, session.ID(id))
		if err != nil {
			return err
		}
		encoded, err := json.Marshal(value)
		if err != nil {
			return err
		}
		_, err = os.Stdout.Write(append(encoded, '\n'))
		return err
	}
	value := archive{}
	value.Snapshot, err = store.Inspect(ctx, session.ID(id))
	if err != nil {
		return err
	}
	var after sessionstore.Sequence
	for {
		page, err := store.Items(ctx, session.ID(id), after, 256)
		if err != nil {
			return err
		}
		value.Items = append(value.Items, page.Items...)
		if !page.More {
			break
		}
		after = page.NextAfter
	}
	resumed, err := store.Resume(ctx, session.ID(id))
	if err != nil {
		return err
	}
	value.Operations, value.ExternalInputIDs = resumed.Operations, resumed.ExternalInputIDs
	encoded, err := json.Marshal(value)
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(append(encoded, '\n'))
	return err
}
