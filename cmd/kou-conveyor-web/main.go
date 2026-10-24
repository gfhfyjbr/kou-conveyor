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

	"golang.org/x/term"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/canvascli"
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
	// canvas runs the canvases (canvas.go).
	canvas bool
	// tunnel is whether the cockpit is carried through its relay from the
	// start (tunnel.go): on, off, or auto, as it was when it last ran.
	tunnel string
	// detach runs agents apart from the server (detached.go): their runners
	// go on when it shuts down or takes a new build up, and the server that
	// starts next takes their runs up.
	detach bool
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
		"run from a checkout, build the server anew when its Go code changes and take the build up in place, once no agent runs attached to it (also KOU_CONVEYOR_WEB_REBUILD=0)")
	f.BoolVar(&o.detach, "detach", envOr("KOU_CONVEYOR_WEB_DETACH", "1") != "0",
		"run agents apart from the server: they go on when it shuts down or takes a new build up, and the server that starts next takes them up (also KOU_CONVEYOR_WEB_DETACH=0)")
	f.StringVar(&assetsFlag, "assets", envOr("KOU_CONVEYOR_WEB_ASSETS", assetsAuto),
		"the page and built-in plugins: auto (live from the checkout the program was built from, while it is there, else compiled in), embedded, or a directory holding static/ and plugins/ (also KOU_CONVEYOR_WEB_ASSETS)")
	canvasFlag := f.String("canvas", envOr("KOU_CONVEYOR_CANVAS", "on"),
		"the canvases: boards of terminals and agents wired together, which agents may build too: on or off (also KOU_CONVEYOR_CANVAS)")
	f.StringVar(&o.tunnel, "tunnel", envOr("KOU_CONVEYOR_TUNNEL", "auto"),
		"carry the cockpit through a relay on another host to the iPhone app and browsers elsewhere: on (from the start), off (until /tunnel starts it) or auto (as it was when the cockpit last ran) (also KOU_CONVEYOR_TUNNEL); /tunnel in the cockpit, or kou-conveyor-web tunnel, sets the relay up")
	if err := f.Parse(args); err != nil {
		return o, err
	}
	if f.NArg() != 0 {
		return o, errors.New("unexpected arguments")
	}
	switch o.tunnel = strings.ToLower(strings.TrimSpace(o.tunnel)); o.tunnel {
	case "on", "off", "auto":
	case "1", "true":
		o.tunnel = "on"
	case "0", "false":
		o.tunnel = "off"
	default:
		return o, errors.New("tunnel must be on, off or auto")
	}
	switch strings.ToLower(strings.TrimSpace(*canvasFlag)) {
	case "on", "1", "true":
		o.canvas = true
	case "off", "0", "false":
	default:
		return o, errors.New("canvas must be on or off")
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

func main() {
	// Called kou-canvas — the link the server puts on the PATH of the
	// canvases' nodes, to itself when kou-conveyor-canvas is not beside
	// it — it is kou-canvas.
	switch filepath.Base(os.Args[0]) {
	case "kou-canvas", "kou-conveyor-canvas":
		os.Exit(canvascli.Main(os.Args[1:]))
	}
	os.Exit(runMain(os.Args[1:]))
}

func runMain(args []string) int {
	if len(args) > 0 && args[0] == "tunnel" {
		return tunnelMain(args[1:], os.Stdout, os.Stderr)
	}
	if len(args) > 0 && args[0] == "metrics" {
		return metricsMain(args[1:], os.Stdout, os.Stderr)
	}
	o, err := parseOptions(args, os.Stderr)
	if errors.Is(err, flag.ErrHelp) {
		return 0
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web:", err)
		return 2
	}
	tuneMemory(os.Getenv)
	if o.Runner, err = cockpit.LocateRunner(o.Runner); err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web:", err)
		return 1
	}
	// The pages take the pictures the agent looked at from the session
	// files (handleToolImage): the transcripts here keep none.
	cockpit.KeepPictures = false
	// An interrupt or SIGTERM shuts the server down. The agents whose runners
	// run apart from it go on, unless a second interrupt from the terminal
	// stops them too (shutDown).
	ctx, stop := context.WithCancel(context.Background())
	defer stop()
	signals := make(chan os.Signal, 2)
	signal.Notify(signals, os.Interrupt, syscall.SIGTERM)
	defer signal.Stop(signals)
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
	handler := s.handler()
	server := &http.Server{
		Handler:           handler,
		BaseContext:       func(net.Listener) context.Context { return ctx },
		ReadHeaderTimeout: 10 * time.Second,
		IdleTimeout:       2 * time.Minute,
	}
	// Shutdown waits for none of the streams read over WebSockets: they
	// are not its connections any more (eventsocket.go).
	server.RegisterOnShutdown(s.sockets.drop)
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
	if s.adopted > 0 {
		fmt.Printf("agents            %s taken up, which went on while the server was away\n", runsOf(s.adopted))
	}
	if dir := sessionLinksDir(o.SettingsFile); dir != "" {
		fmt.Printf("sessions          %s (every workspace's, linked, with index.json)\n", display(dir))
		go s.keepSessionLinks(ctx, &sessionLinks{dir: dir})
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
	// The tunnel serves what the pages here are served, through its relay.
	s.tunnel.start(ctx, handler)
	if status := s.tunnel.view(false); status.Wanted {
		where := status.URL
		if where == "" {
			where = "waiting for a relay to be set up (/tunnel)"
		}
		fmt.Printf("tunnel            %s\n", where)
	}

	var first os.Signal
	select {
	case err := <-served:
		fmt.Fprintln(os.Stderr, "kou-conveyor-web:", err)
		return 1
	case first = <-signals:
	}
	// From a terminal, a second Ctrl+C stops the agents that go on; a
	// service manager's signals leave them be.
	interactive := first == os.Interrupt && term.IsTerminal(int(os.Stdin.Fd()))
	return shutDown(s, server, stop, signals, interactive)
}

// shutDown shuts the server down once it got a signal: the runners attached
// to it are stopped, those apart from it go on, and the server that starts
// next takes their runs up. Interactive, the server stops them as well when
// it is interrupted again within a few seconds.
func shutDown(s *server, web *http.Server, stop context.CancelFunc, signals chan os.Signal, interactive bool) int {
	going := s.detachedRuns()
	stop()
	them := "them"
	if len(going) == 1 {
		them = "it"
	}
	var again <-chan time.Time
	switch {
	case len(going) == 0:
		signal.Stop(signals) // a second interrupt terminates immediately
		fmt.Println("shutting down: stopping runs…")
	case interactive:
		fmt.Printf("shutting down: %s in the background; the server takes %s up when it starts again\n", goOn(len(going)), them)
		fmt.Printf("press Ctrl+C again within 5 seconds to stop %s\n", them)
		again = time.After(5 * time.Second)
	default:
		signal.Stop(signals)
		fmt.Printf("shutting down: %s in the background; the server takes %s up when it starts again\n", goOn(len(going)), them)
	}
	shutdown, cancel := context.WithTimeout(context.Background(), 20*time.Second)
	defer cancel()
	_ = web.Shutdown(shutdown)
	code := 0
	if !s.close(shutdown) {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: some runners did not stop in time")
		code = 1
	}
	if again == nil {
		return code
	}
	stopThem := false
	select {
	case <-signals:
		stopThem = true
	default:
		select {
		case <-signals:
			stopThem = true
		case <-again:
		}
	}
	signal.Stop(signals) // a third interrupt terminates immediately
	if !stopThem {
		return code
	}
	fmt.Printf("stopping %s…\n", runsOf(len(going)))
	stopping, cancelStopping := context.WithTimeout(context.Background(), 30*time.Second)
	defer cancelStopping()
	if err := stopRuns(stopping, going, "the user stopped the server and its agents"); err != nil {
		fmt.Fprintln(os.Stderr, "kou-conveyor-web: some agents did not stop:", err)
		return 1
	}
	return code
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
