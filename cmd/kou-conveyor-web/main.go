// Command kou-conveyor-web serves a browser cockpit for kou-conveyor-runner.
// The runner remains the source of truth for execution and durable sessions;
// this process launches it and brokers its events to local browsers.
package main

import (
	"context"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

type options struct {
	cockpit.Options
	address, model, thinking string
	allowedHosts             []string
	// accounts is where the accounts gateway listens, "off" or "" for
	// none; accountsDir holds its configuration and credentials.
	accounts, accountsDir string
	// assets are the page and the built-in plugins (assets.go); nil serves
	// those compiled in.
	assets *assets
	// rebuild has a server that runs from a checkout build itself anew as
	// its Go code changes, and take the new build up in place (rebuild.go).
	rebuild bool
}

func parseOptions(args []string, output io.Writer) (options, error) {
	var o options
	var hosts, assetsFlag string
	f := flag.NewFlagSet("kou-conveyor-web", flag.ContinueOnError)
	f.SetOutput(output)
	f.StringVar(&o.Workspace, "workspace", ".", "agent workspace")
	f.StringVar(&o.address, "address", "127.0.0.1:8080", "HTTP listen address")
	f.StringVar(&o.Runner, "runner", "", "runner executable (also KOU_CONVEYOR_RUNNER)")
	f.StringVar(&o.SessionDir, "session-directory", "", "runner sessions (default: <workspace>/.harness/sessions)")
	f.StringVar(&o.LogDir, "log-directory", "", "runner JSONL log directory")
	f.StringVar(&o.model, "model", "", "model ID; otherwise use runner environment/default")
	f.StringVar(&o.Provider, "provider", "", "provider override")
	f.StringVar(&o.thinking, "thinking-level", "high", "default thinking level: low, medium, high, xhigh, max")
	f.StringVar(&hosts, "allowed-hosts", "", "extra comma-separated Host names to accept, e.g. behind a reverse proxy")
	f.StringVar(&o.SettingsFile, "config", "", "connection settings file (default: KOU_CONVEYOR_CONFIG or the user config directory)")
	f.DurationVar(&o.Heartbeat, "tool-heartbeat-interval", 10*time.Minute, "runner tool-wait heartbeat (0 disables)")
	f.StringVar(&o.accounts, "accounts", "127.0.0.1:8318", "address of the accounts gateway (CLIProxyAPI) that serves runs from signed-in subscriptions; off disables it")
	f.StringVar(&o.accountsDir, "accounts-dir", "", "the gateway's folder: config.yaml, credentials and history (default: cliproxy beside the settings)")
	f.BoolVar(&o.rebuild, "rebuild", envOr("KOU_CONVEYOR_WEB_REBUILD", "1") != "0",
		"run from a checkout, build the server anew when its Go code changes and take the build up in place, once no agent runs (also KOU_CONVEYOR_WEB_REBUILD=0)")
	f.StringVar(&assetsFlag, "assets", envOr("KOU_CONVEYOR_WEB_ASSETS", assetsAuto),
		"the page and built-in plugins: auto (live from the checkout the program was built from, while it is there, else compiled in), embedded, or a directory holding static/ and plugins/ (also KOU_CONVEYOR_WEB_ASSETS)")
	if err := f.Parse(args); err != nil {
		return o, err
	}
	if f.NArg() != 0 {
		return o, errors.New("unexpected arguments")
	}
	if o.Heartbeat < 0 {
		return o, errors.New("tool-heartbeat-interval must not be negative")
	}
	var err error
	if o.assets, err = resolveAssets(strings.TrimSpace(assetsFlag)); err != nil {
		return o, err
	}
	if !cockpit.ValidThinkingLevel(o.thinking) {
		return o, errors.New("thinking-level must be low, medium, high, xhigh or max")
	}
	if o.accounts = strings.TrimSpace(o.accounts); o.accounts != "" && o.accounts != "off" {
		if err := accounts.ValidAddress(o.accounts); err != nil {
			return o, err
		}
	}
	for host := range strings.SplitSeq(hosts, ",") {
		if host = strings.TrimSpace(host); host != "" {
			o.allowedHosts = append(o.allowedHosts, strings.ToLower(host))
		}
	}
	if o.Workspace, err = filepath.Abs(o.Workspace); err != nil {
		return o, err
	}
	if info, err := os.Stat(o.Workspace); err != nil || !info.IsDir() {
		return o, fmt.Errorf("workspace is not a directory: %s", o.Workspace)
	}
	if o.SessionDir == "" {
		o.SessionDir = filepath.Join(o.Workspace, ".harness", "sessions")
	}
	if o.SessionDir, err = filepath.Abs(o.SessionDir); err != nil {
		return o, err
	}
	if o.LogDir != "" {
		if o.LogDir, err = filepath.Abs(o.LogDir); err != nil {
			return o, err
		}
	}
	if o.accountsDir != "" {
		if o.accountsDir, err = filepath.Abs(o.accountsDir); err != nil {
			return o, err
		}
	}
	if o.SettingsFile == "" {
		// Without a home directory there is nowhere to keep settings; the
		// environment still configures runs.
		o.SettingsFile, _ = cockpit.SettingsPath()
	} else if o.SettingsFile, err = filepath.Abs(o.SettingsFile); err != nil {
		return o, err
	}
	return o, nil
}

func main() { os.Exit(runMain(os.Args[1:])) }

func runMain(args []string) int {
	o, err := parseOptions(args, os.Stderr)
	if errors.Is(err, flag.ErrHelp) {
		return 0
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web:", err)
		return 2
	}
	if o.Runner, err = cockpit.LocateRunner(o.Runner); err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web:", err)
		return 1
	}
	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	// A build before this one hands over its socket: pages that were open
	// reconnect to this one.
	listener, err := inheritedListener()
	restarted := listener != nil
	if err == nil && listener == nil {
		listener, err = net.Listen("tcp", o.address)
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web:", err)
		return 1
	}
	s := newServer(ctx, o, listener.Addr())
	server := &http.Server{
		Handler:           s.handler(),
		BaseContext:       func(net.Listener) context.Context { return ctx },
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       2 * time.Minute,
	}
	served := make(chan error, 1)
	go func() { served <- server.Serve(listener) }()

	if restarted {
		fmt.Printf("kou-conveyor-web  http://%s  restarted with the new build\n", displayAddress(listener.Addr()))
	} else {
		fmt.Printf("kou-conveyor-web  http://%s\n", displayAddress(listener.Addr()))
	}
	fmt.Printf("workspace         %s\n", o.Workspace)
	fmt.Printf("interface         %s\n", s.assets.describe())
	if s.rebuild != nil {
		fmt.Printf("go code           %s (built anew as it changes)\n", display(s.rebuild.root))
		go s.followGo(ctx, listener)
	}
	if n := len(s.workspaces.all()) - 1; n > 0 {
		fmt.Printf("                  and %d more added in the browser\n", n)
	}
	if s.gateway != nil {
		status := s.gateway.Status()
		fmt.Printf("accounts          %s  (CLIProxyAPI %s, %s)\n", status.URL, status.Version, display(status.Dir))
	} else if s.gatewayErr != "" {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: accounts gateway:", s.gatewayErr)
	}
	if s.anyHost {
		fmt.Println("warning: listening on all interfaces; anyone who can reach this port can run commands in the workspace")
	}

	select {
	case err := <-served:
		fmt.Fprintln(os.Stderr, "kou-conveyor-web:", err)
		return 1
	case <-ctx.Done():
	}
	stop() // a second interrupt terminates immediately
	fmt.Println("shutting down: stopping runs…")
	shutdown, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	_ = server.Shutdown(shutdown)
	if !s.close(shutdown) {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: some runners did not stop in time")
		return 1
	}
	return 0
}

func envOr(name, fallback string) string {
	if value := strings.TrimSpace(os.Getenv(name)); value != "" {
		return value
	}
	return fallback
}

func displayAddress(addr net.Addr) string {
	if tcp, ok := addr.(*net.TCPAddr); ok && tcp.IP.IsUnspecified() {
		return net.JoinHostPort("127.0.0.1", strconv.Itoa(tcp.Port))
	}
	return addr.String()
}
