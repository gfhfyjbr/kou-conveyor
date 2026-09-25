// Command kou-conveyor-runner executes one JSON request and writes persisted session items as JSONL.
package main

import (
	"context"
	"os"
	"os/signal"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/agentrunner"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
)

func main() {
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()
	os.Exit(agentrunner.RunMain(
		ctx, os.Args[1:], os.Getenv, os.Environ,
		os.Stdin, os.Stdout, os.Stderr,
		agentrunner.Config{
			Name:         "kou-conveyor-runner",
			ParseRequest: parseRequest,
			Providers:    agentrunner.DefaultProviders(),
			// The user's plugins live beside the cockpits' settings.
			PluginConfigDirectory: plugin.ConfigDirectory,
		},
	))
}
