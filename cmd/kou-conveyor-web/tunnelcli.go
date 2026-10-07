package main

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"errors"
	"flag"
	"fmt"
	"io"
	"net"
	"net/url"
	"os"
	"os/exec"
	"path/filepath"
	"regexp"
	"strconv"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/tunnel"
	"rsc.io/qr"
)

// kou-conveyor-web tunnel: the tunnel's commands, for the user at a
// terminal and for the agent that /tunnel in the cockpit hands the setup to.

const tunnelUsage = `Usage: kou-conveyor-web tunnel <command> [options]

The tunnel carries the cockpit through a relay, kou-conveyor-relay, on a host
the iPhone app and browsers elsewhere reach. The cockpit connects out to the
relay; tunnel.json says where it is. A cockpit that runs follows tunnel.json:
what these commands write there it takes up within seconds.

Commands:
  status         where the relay is, and whether it answers (also: check)
  install DEST   install the relay on DEST over ssh (user@host, or a Host of
                 ~/.ssh/config), keep it running as a service, and set the
                 tunnel up to it
  set            set the tunnel up to a relay by hand: -url, -token, -fingerprint
  start, stop    have the cockpit carried through the relay, or no longer
  link           the link that pairs the iPhone app, and its QR code
                 (-browser: the link that opens the cockpit in a browser)

Each command takes -config FILE, the tunnel.json (default:
KOU_CONVEYOR_TUNNEL_CONFIG, or beside the cockpit's settings), and -h for
the rest of its options.
`

func tunnelMain(args []string, stdout, stderr io.Writer) int {
	if len(args) == 0 {
		fmt.Fprint(stderr, tunnelUsage)
		return 2
	}
	command, rest := args[0], args[1:]
	switch command {
	case "status", "check":
		return tunnelStatusCommand(rest, stdout, stderr)
	case "install":
		return tunnelInstallCommand(rest, stdout, stderr)
	case "set":
		return tunnelSetCommand(rest, stdout, stderr)
	case "start", "stop":
		return tunnelEnableCommand(command, rest, stdout, stderr)
	case "link":
		return tunnelLinkCommand(rest, stdout, stderr)
	case "-h", "-help", "--help", "help":
		fmt.Fprint(stdout, tunnelUsage)
		return 0
	}
	fmt.Fprintf(stderr, "kou-conveyor-web tunnel: no command %q\n\n%s", command, tunnelUsage)
	return 2
}

// tunnelFlags is the flag set of a tunnel command, with -config.
func tunnelFlags(name string, stderr io.Writer) (*flag.FlagSet, *string) {
	f := flag.NewFlagSet("kou-conveyor-web tunnel "+name, flag.ContinueOnError)
	f.SetOutput(stderr)
	path := f.String("config", defaultTunnelConfig(), "the tunnel's configuration, tunnel.json")
	return f, path
}

// parseTunnelFlags parses a command's flags; done says the command ends, with code.
func parseTunnelFlags(f *flag.FlagSet, args []string, operands int) (code int, done bool) {
	if err := f.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return 0, true
		}
		return 2, true
	}
	if f.NArg() != operands {
		fmt.Fprintf(f.Output(), "%s: expected %d arguments, not %d\n", f.Name(), operands, f.NArg())
		f.Usage()
		return 2, true
	}
	return 0, false
}

func defaultTunnelConfig() string {
	settings, _ := cockpit.SettingsPath()
	return tunnelConfigPath(settings)
}

// tunnelReport is what status says.
type tunnelReport struct {
	OK          bool   `json:"ok"`
	Code        string `json:"code,omitempty"`
	Error       string `json:"error,omitempty"`
	Hint        string `json:"hint,omitempty"`
	Config      string `json:"config"`
	URL         string `json:"url,omitempty"`
	Host        string `json:"host,omitempty"`
	Fingerprint string `json:"fingerprint,omitempty"`
	// Enabled is that the cockpit is carried through the relay as it runs.
	Enabled bool `json:"enabled"`
	// The relay's version, and whether a cockpit is connected to it now.
	Relay     string `json:"relay_version,omitempty"`
	Connected bool   `json:"cockpit_connected"`
}

func checkTunnel(path string) tunnelReport {
	report := tunnelReport{Config: path}
	c, err := tunnel.LoadConfig(path)
	if err != nil {
		err = notConfigured(path, err)
		report.Code, report.Error = tunnel.Code(err), err.Error()
		report.Hint = tunnelHint(report.Code)
		return report
	}
	report.URL, report.Host, report.Fingerprint, report.Enabled = c.Base(), c.Host, c.Fingerprint, c.Enabled
	ctx, cancel := context.WithTimeout(context.Background(), 10*time.Second)
	defer cancel()
	health, err := tunnel.Check(ctx, c)
	if err != nil {
		report.Code, report.Error = tunnel.Code(err), err.Error()
		report.Hint = tunnelHint(report.Code)
		return report
	}
	report.OK, report.Relay, report.Connected = true, health.Version, health.Agent
	return report
}

func (r tunnelReport) print(w io.Writer) {
	fmt.Fprintf(w, "config    %s\n", display(r.Config))
	if r.URL != "" {
		where := r.URL
		if r.Host != "" {
			where += "  (installed on " + r.Host + ")"
		}
		fmt.Fprintf(w, "relay     %s\n", where)
	}
	if r.OK {
		state := "answers"
		if r.Relay != "" {
			state += " (" + tunnel.Name + " " + r.Relay + ")"
		}
		if r.Connected {
			state += "; a cockpit is connected to it"
		} else {
			state += "; no cockpit is connected to it"
		}
		fmt.Fprintf(w, "state     %s\n", state)
	} else {
		fmt.Fprintf(w, "error     %s [%s]\n", r.Error, r.Code)
	}
	if r.URL != "" {
		if r.Enabled {
			fmt.Fprintln(w, "tunnel    on: the cockpit is carried through the relay while it runs")
		} else {
			fmt.Fprintln(w, "tunnel    off: kou-conveyor-web tunnel start, or /tunnel in the cockpit, turns it on")
		}
	}
	if r.Hint != "" {
		fmt.Fprintf(w, "hint      %s\n", r.Hint)
	}
}

// tunnelHint says what to do about an error of the tunnel, by its code.
func tunnelHint(code string) string {
	switch code {
	case "not_configured":
		return "install a relay: kou-conveyor-web tunnel install user@host"
	case "unreachable":
		return "is the relay running on its host (systemctl --user status kou-conveyor-relay), and its port open in the host's firewall and in its provider's security group?"
	case "unauthorized":
		return "tunnel.json's token is not the relay's: install it again, or set -token to the host's ~/.config/kou-conveyor-relay/token"
	case "not_relay":
		return "something else answers at that address: check the url and the port"
	case "incompatible":
		return "the relay and this cockpit are of different versions: install the relay again"
	}
	return ""
}

func writeTunnelJSON(w io.Writer, v any) {
	data, _ := json.Marshal(v)
	fmt.Fprintln(w, string(data))
}

func tunnelStatusCommand(args []string, stdout, stderr io.Writer) int {
	f, path := tunnelFlags("status", stderr)
	asJSON := f.Bool("json", false, "answer in JSON")
	if code, done := parseTunnelFlags(f, args, 0); done {
		return code
	}
	report := checkTunnel(*path)
	if *asJSON {
		writeTunnelJSON(stdout, report)
	} else {
		report.print(stdout)
	}
	if !report.OK {
		return 1
	}
	return 0
}

func tunnelSetCommand(args []string, stdout, stderr io.Writer) int {
	f, path := tunnelFlags("set", stderr)
	rawURL := f.String("url", "", "the relay's URL: https://host:port")
	token := f.String("token", "", "the relay's token")
	fingerprint := f.String("fingerprint", "", "the SHA-256 of the relay's own certificate, which pins it (kou-conveyor-relay -print-config says it); empty for a certificate a CA vouches for")
	host := f.String("host", "", "the ssh destination the relay runs on, for the record")
	enable := f.Bool("enable", true, "turn the tunnel on: the cockpit connects to the relay")
	asJSON := f.Bool("json", false, "answer in JSON")
	if code, done := parseTunnelFlags(f, args, 0); done {
		return code
	}
	given := map[string]bool{}
	f.Visit(func(flag *flag.Flag) { given[flag.Name] = true })
	// What is not given stays as tunnel.json says.
	c, _ := tunnel.LoadConfig(*path)
	if given["url"] {
		c.URL = strings.TrimRight(strings.TrimSpace(*rawURL), "/")
	}
	if given["token"] {
		c.Token = strings.TrimSpace(*token)
	}
	if given["fingerprint"] {
		c.Fingerprint = ""
		if *fingerprint != "" {
			fp, err := tunnel.NormalizeFingerprint(*fingerprint)
			if err != nil {
				fmt.Fprintln(stderr, "kou-conveyor-web tunnel set:", err)
				return 2
			}
			c.Fingerprint = fp
		}
	}
	if given["host"] {
		c.Host = strings.TrimSpace(*host)
	}
	c.Enabled = *enable
	if err := tunnel.SaveConfig(*path, c); err != nil {
		fmt.Fprintln(stderr, "kou-conveyor-web tunnel set:", err)
		return 1
	}
	report := checkTunnel(*path)
	if *asJSON {
		writeTunnelJSON(stdout, report)
	} else {
		fmt.Fprintln(stdout, "saved "+display(*path))
		report.print(stdout)
	}
	if !report.OK {
		return 1
	}
	return 0
}

func tunnelEnableCommand(name string, args []string, stdout, stderr io.Writer) int {
	f, path := tunnelFlags(name, stderr)
	if code, done := parseTunnelFlags(f, args, 0); done {
		return code
	}
	c, err := tunnel.LoadConfig(*path)
	if err != nil {
		err = notConfigured(*path, err)
		fmt.Fprintf(stderr, "kou-conveyor-web tunnel %s: %v\n%s\n", name, err, tunnelHint(tunnel.Code(err)))
		return 1
	}
	c.Enabled = name == "start"
	if err := tunnel.SaveConfig(*path, c); err != nil {
		fmt.Fprintf(stderr, "kou-conveyor-web tunnel %s: %v\n", name, err)
		return 1
	}
	if c.Enabled {
		fmt.Fprintln(stdout, "on: a cockpit that runs connects to "+c.Base()+" within seconds; one that starts, at once")
	} else {
		fmt.Fprintln(stdout, "off: a cockpit that runs lets the relay go within seconds")
	}
	return 0
}

func tunnelLinkCommand(args []string, stdout, stderr io.Writer) int {
	f, path := tunnelFlags("link", stderr)
	browser := f.Bool("browser", false, "the link that opens the cockpit in a browser, instead of the one that pairs the iPhone app")
	drawQR := f.Bool("qr", isTerminal(stdout), "draw the link as a QR code too (by default when the output is a terminal)")
	if code, done := parseTunnelFlags(f, args, 0); done {
		return code
	}
	c, err := tunnel.LoadConfig(*path)
	if err != nil {
		err = notConfigured(*path, err)
		fmt.Fprintf(stderr, "kou-conveyor-web tunnel link: %v\n%s\n", err, tunnelHint(tunnel.Code(err)))
		return 1
	}
	link := c.PairLink()
	if *browser {
		link = c.BrowserLink()
	}
	if *drawQR {
		if err := printQR(stdout, link); err != nil {
			fmt.Fprintln(stderr, "kou-conveyor-web tunnel link:", err)
			return 1
		}
	}
	fmt.Fprintln(stdout, link)
	return 0
}

func isTerminal(w io.Writer) bool {
	file, ok := w.(*os.File)
	if !ok {
		return false
	}
	info, err := file.Stat()
	return err == nil && info.Mode()&os.ModeCharDevice != 0
}

// printQR draws a QR code in half blocks, two modules a character: dark on
// light whatever the terminal's colors, with a quiet zone around.
func printQR(w io.Writer, text string) error {
	code, err := qr.Encode(text, qr.L)
	if err != nil {
		return err
	}
	const quiet = 2
	size := code.Size + 2*quiet
	dark := func(x, y int) bool {
		x, y = x-quiet, y-quiet
		return x >= 0 && y >= 0 && x < code.Size && y < code.Size && code.Black(x, y)
	}
	var b strings.Builder
	for y := 0; y < size; y += 2 {
		b.WriteString("\x1b[30;47m")
		for x := range size {
			switch top, bottom := dark(x, y), dark(x, y+1); {
			case top && bottom:
				b.WriteString("█")
			case top:
				b.WriteString("▀")
			case bottom:
				b.WriteString("▄")
			default:
				b.WriteString(" ")
			}
		}
		b.WriteString("\x1b[0m\n")
	}
	_, err = io.WriteString(w, b.String())
	return err
}

// ---------------------------------------------------------------- install

// relayInstall installs the relay on a host over ssh: it looks at the host,
// builds the relay for it from the checkout, uploads it, has it make its
// token and certificate, keeps it running as a service, and writes
// tunnel.json.
type relayInstall struct {
	dest    string
	ssh     []string
	port    int
	tls     string
	url     string
	binary  string
	service string
	out     io.Writer

	// What the host said of itself.
	goos, goarch, goarm string
	root, systemd       bool
	installed           string
}

// installReport is the JSON -json ends the install with. It holds no token.
type installReport struct {
	OK          bool   `json:"ok"`
	Code        string `json:"code,omitempty"`
	Error       string `json:"error,omitempty"`
	Hint        string `json:"hint,omitempty"`
	Saved       bool   `json:"saved"`
	Config      string `json:"config"`
	URL         string `json:"url,omitempty"`
	Host        string `json:"host"`
	Platform    string `json:"platform,omitempty"`
	Service     string `json:"service,omitempty"`
	Fingerprint string `json:"fingerprint,omitempty"`
}

var safeHostName = regexp.MustCompile(`^[A-Za-z0-9._:-]+$`)

func tunnelInstallCommand(args []string, stdout, stderr io.Writer) int {
	f, path := tunnelFlags("install", stderr)
	port := f.Int("port", tunnel.DefaultPort, "the port the relay listens on")
	tlsMode := f.String("tls", "self-signed", "self-signed: a certificate of the relay's own, which the cockpit and the app pin; off: plain HTTP, only behind a TLS proxy or on a private network such as Tailscale")
	publicURL := f.String("url", "", "the relay's URL as the phone and the cockpit reach it (default: http(s)://<the host's name>:<port>)")
	binary := f.String("binary", "", "a kou-conveyor-relay built for the host, to upload (default: built from the checkout of kou-conveyor)")
	service := f.String("service", "auto", "how the relay keeps running: auto (systemd if the host has it, else nohup), systemd, nohup, or none")
	sshOptions := f.String("ssh", "", `more options for ssh, in one argument: "-p 2222 -i ~/.ssh/relay"`)
	dryRun := f.Bool("dry-run", false, "look at the host and say what would be done; change nothing")
	asJSON := f.Bool("json", false, "end with a report in JSON")
	f.Usage = func() {
		fmt.Fprint(stderr, `Usage: kou-conveyor-web tunnel install [options] DEST

Installs kou-conveyor-relay on DEST — user@host, or a Host of ~/.ssh/config —
over ssh, which must log in without a password (a key, an agent). The relay
goes to ~/.local/bin there and keeps its token and certificate in
~/.config/kou-conveyor-relay; it is kept running as a systemd service, or
with nohup. tunnel.json is written then, and the cockpit connects.

Its port must be open to the phone: in the host's firewall, and in its
provider's security group.

`)
		f.PrintDefaults()
	}
	if code, done := parseTunnelFlags(f, args, 1); done {
		return code
	}
	in := &relayInstall{
		dest: f.Arg(0), ssh: strings.Fields(*sshOptions), port: *port, tls: *tlsMode,
		url: strings.TrimRight(strings.TrimSpace(*publicURL), "/"), binary: *binary, service: *service, out: stdout,
	}
	report := installReport{Config: *path, Host: in.dest}
	fail := func(code string, err error) int {
		report.Code, report.Error = code, err.Error()
		if report.Hint == "" {
			report.Hint = tunnelHint(code)
		}
		if *asJSON {
			writeTunnelJSON(stdout, report)
		} else {
			fmt.Fprintf(stderr, "kou-conveyor-web tunnel install: %v\n", err)
			if report.Hint != "" {
				fmt.Fprintln(stderr, "hint: "+report.Hint)
			}
		}
		if report.Saved {
			return 3
		}
		return 1
	}
	if err := in.validate(); err != nil {
		fmt.Fprintln(stderr, "kou-conveyor-web tunnel install:", err)
		return 2
	}

	if err := in.probe(); err != nil {
		report.Hint = "ssh must log in to " + in.dest + " without asking anything: a key in ~/.ssh or in the ssh agent, and the host known or new"
		return fail("ssh", err)
	}
	report.Platform = in.goos + "/" + in.goarch
	mode := in.chooseService()
	report.Service = mode
	relayURL, err := in.relayURL()
	if err != nil {
		return fail("not_configured", err)
	}
	report.URL = relayURL
	if *dryRun {
		in.step("would build %s for %s, upload it to %s:~/.local/bin, run it on port %d (tls %s) %s, and save %s for %s",
			tunnel.Name, report.Platform, in.dest, in.port, in.tls, serviceSays(mode, in.root), display(*path), relayURL)
		if *asJSON {
			report.OK = true
			writeTunnelJSON(stdout, report)
		}
		return 0
	}

	binaryPath := in.binary
	if binaryPath == "" {
		source, err := relaySource()
		if err != nil {
			return fail("build", err)
		}
		in.step("build %s for %s from %s", tunnel.Name, report.Platform, display(source))
		built, cleanup, err := buildRelay(source, in.goos, in.goarch, in.goarm)
		if err != nil {
			return fail("build", err)
		}
		defer cleanup()
		binaryPath = built
	}
	if err := in.upload(binaryPath); err != nil {
		return fail("upload", err)
	}
	printed, err := in.printConfig(relayURL)
	if err != nil {
		return fail("relay", err)
	}
	report.Fingerprint = printed.Fingerprint
	if mode != "none" {
		if mode, err = in.keepRunning(mode, relayURL); err != nil {
			return fail("service", err)
		}
		report.Service = mode
	}

	config := tunnel.Config{URL: relayURL, Token: printed.Token, Fingerprint: printed.Fingerprint, Host: in.dest, Enabled: true}
	if err := tunnel.SaveConfig(*path, config); err != nil {
		return fail("not_configured", err)
	}
	report.Saved = true
	in.step("saved %s: a cockpit that runs connects to the relay within seconds", display(*path))

	if mode == "none" {
		report.OK = true
		in.step("the relay is installed, not started: run it on the host as %s", in.relayCommand(relayURL, false))
	} else if err := in.await(config); err != nil {
		report.Hint = in.diagnose(relayURL)
		return fail(tunnel.Code(err), err)
	} else {
		report.OK = true
		in.step("the relay answers at %s", relayURL)
	}
	if *asJSON {
		writeTunnelJSON(stdout, report)
	} else {
		fmt.Fprintln(stdout, "done: pair the iPhone app with kou-conveyor-web tunnel link, or in the cockpit's tunnel dialog")
		if in.tls == "self-signed" {
			fmt.Fprintln(stdout, "browsers warn of the relay's own certificate once; the app and the cockpit pin it")
		}
	}
	return 0
}

func (in *relayInstall) validate() error {
	switch {
	case in.dest == "" || strings.HasPrefix(in.dest, "-") || strings.ContainsAny(in.dest, " \t\r\n'\"\\;|&$`<>(){}*?!#"):
		return fmt.Errorf("%q is not an ssh destination: user@host, or a Host of ~/.ssh/config", in.dest)
	case in.port < 1 || in.port > 65535:
		return fmt.Errorf("the port must be 1 to 65535, not %d", in.port)
	case in.tls != "self-signed" && in.tls != "off":
		return fmt.Errorf("-tls must be self-signed or off, not %q", in.tls)
	}
	switch in.service {
	case "auto", "systemd", "nohup", "none":
	default:
		return fmt.Errorf("-service must be auto, systemd, nohup or none, not %q", in.service)
	}
	if in.binary != "" {
		if _, err := os.Stat(in.binary); err != nil {
			return err
		}
	}
	return nil
}

func (in *relayInstall) step(format string, args ...any) {
	fmt.Fprintf(in.out, "→ %s\n", fmt.Sprintf(format, args...))
}

// remote runs a POSIX shell script on the host; stdin, when given, is the
// script's.
func (in *relayInstall) remote(script string, stdin io.Reader) (string, error) {
	args := []string{"-o", "BatchMode=yes", "-o", "ConnectTimeout=15", "-o", "StrictHostKeyChecking=accept-new"}
	args = append(args, in.ssh...)
	args = append(args, in.dest, "sh -c "+shellQuote(script))
	command := exec.Command("ssh", args...)
	command.Stdin = stdin
	var stdout, stderr bytes.Buffer
	command.Stdout, command.Stderr = &stdout, &stderr
	if err := command.Run(); err != nil {
		message := strings.TrimSpace(stderr.String())
		if message == "" {
			message = strings.TrimSpace(stdout.String())
		}
		if message == "" {
			message = err.Error()
		}
		return stdout.String(), fmt.Errorf("ssh %s: %s", in.dest, message)
	}
	return stdout.String(), nil
}

func shellQuote(s string) string { return "'" + strings.ReplaceAll(s, "'", `'\''`) + "'" }

const probeScript = `uname -s; uname -m; id -u
if command -v systemctl >/dev/null 2>&1 && [ -d /run/systemd/system ]; then echo systemd; else echo none; fi
"$HOME/.local/bin/kou-conveyor-relay" -version 2>/dev/null || echo none`

// probe asks the host what it is.
func (in *relayInstall) probe() error {
	out, err := in.remote(probeScript, nil)
	if err != nil {
		return err
	}
	lines := strings.Split(strings.TrimSpace(out), "\n")
	if len(lines) < 5 {
		return fmt.Errorf("the host's answer is not understood: %q", out)
	}
	system, machine := strings.TrimSpace(lines[0]), strings.TrimSpace(lines[1])
	switch system {
	case "Linux":
		in.goos = "linux"
	case "Darwin":
		in.goos = "darwin"
	case "FreeBSD":
		in.goos = "freebsd"
	default:
		return fmt.Errorf("the relay is not built for %s hosts", system)
	}
	switch machine {
	case "x86_64", "amd64":
		in.goarch = "amd64"
	case "aarch64", "arm64":
		in.goarch = "arm64"
	case "armv7l", "armv7", "armv6l":
		in.goarch, in.goarm = "arm", machine[4:5]
	case "i386", "i686":
		in.goarch = "386"
	default:
		return fmt.Errorf("the relay is not built for %s machines", machine)
	}
	in.root = strings.TrimSpace(lines[2]) == "0"
	in.systemd = strings.TrimSpace(lines[3]) == "systemd"
	if installed := strings.TrimSpace(lines[4]); installed != "none" {
		in.installed = installed
	}
	says := fmt.Sprintf("%s: %s %s", in.dest, system, machine)
	if in.systemd {
		says += ", systemd"
	}
	if in.installed != "" {
		says += "; there now: " + in.installed
	}
	in.step("%s", says)
	return nil
}

func (in *relayInstall) chooseService() string {
	if in.service == "auto" {
		if in.systemd {
			return "systemd"
		}
		return "nohup"
	}
	return in.service
}

func serviceSays(mode string, root bool) string {
	switch mode {
	case "systemd":
		if root {
			return "as a systemd service"
		}
		return "as a systemd user service"
	case "nohup":
		return "with nohup (until the host restarts)"
	}
	return "not started"
}

// relayURL is the relay's URL as its clients reach it: -url, or the name
// ssh connects to, at the port.
func (in *relayInstall) relayURL() (string, error) {
	if in.url != "" {
		u, err := url.Parse(in.url)
		if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" {
			return "", fmt.Errorf("-url must be http(s)://host:port, not %q", in.url)
		}
		return in.url, nil
	}
	scheme := "https"
	if in.tls == "off" {
		scheme = "http"
	}
	return scheme + "://" + net.JoinHostPort(sshHostname(in.dest, in.ssh), strconv.Itoa(in.port)), nil
}

// sshHostname is the name ssh connects to for dest, as its configuration
// says.
func sshHostname(dest string, options []string) string {
	args := append(append([]string{"-G"}, options...), dest)
	if out, err := exec.Command("ssh", args...).Output(); err == nil {
		for line := range strings.Lines(string(out)) {
			if name, ok := strings.CutPrefix(strings.TrimSpace(line), "hostname "); ok && name != "" {
				return name
			}
		}
	}
	if _, host, ok := strings.Cut(dest, "@"); ok {
		return host
	}
	return dest
}

// relayCommand is the relay's command line on the host: for a shell, or,
// with unit, for a systemd unit.
func (in *relayInstall) relayCommand(relayURL string, unit bool) string {
	home := `"$HOME"`
	if unit {
		home = "%h"
	}
	command := fmt.Sprintf("%s/.local/bin/%s -listen :%d -tls %s -dir %s/.config/%s", home, tunnel.Name, in.port, in.tls, home, tunnel.Name)
	if u, err := url.Parse(relayURL); err == nil && in.tls == "self-signed" && safeHostName.MatchString(u.Hostname()) {
		command += " -names " + u.Hostname()
	}
	return command
}

func (in *relayInstall) upload(binary string) error {
	file, err := os.Open(binary)
	if err != nil {
		return err
	}
	defer file.Close()
	info, _ := file.Stat()
	in.step("upload it to %s:~/.local/bin/%s (%.1f MB)", in.dest, tunnel.Name, float64(info.Size())/(1<<20))
	script := fmt.Sprintf(`set -e
mkdir -p "$HOME/.local/bin"
cat > "$HOME/.local/bin/%[1]s.new"
chmod 755 "$HOME/.local/bin/%[1]s.new"
mv -f "$HOME/.local/bin/%[1]s.new" "$HOME/.local/bin/%[1]s"
"$HOME/.local/bin/%[1]s" -version`, tunnel.Name)
	out, err := in.remote(script, file)
	if err != nil {
		return err
	}
	in.step("installed: %s", strings.TrimSpace(out))
	return nil
}

// relayPrinted is what the relay's -print-config says.
type relayPrinted struct {
	Token       string `json:"token"`
	Fingerprint string `json:"fingerprint"`
	Protocol    int    `json:"protocol"`
	Version     string `json:"version"`
}

// printConfig has the relay make its token and certificate, when it has
// none, and say them.
func (in *relayInstall) printConfig(relayURL string) (relayPrinted, error) {
	var printed relayPrinted
	out, err := in.remote(in.relayCommand(relayURL, false)+" -print-config", nil)
	if err != nil {
		return printed, err
	}
	lines := strings.Split(strings.TrimSpace(out), "\n")
	if err := json.Unmarshal([]byte(lines[len(lines)-1]), &printed); err != nil || printed.Token == "" {
		return printed, fmt.Errorf("the relay did not say its token: %q", out)
	}
	if printed.Protocol != tunnel.Protocol {
		return printed, &tunnel.Error{Code: "incompatible", Message: fmt.Sprintf("the relay speaks protocol %d, this cockpit %d", printed.Protocol, tunnel.Protocol)}
	}
	if in.tls == "self-signed" && printed.Fingerprint == "" {
		return printed, errors.New("the relay said no fingerprint of its certificate")
	}
	return printed, nil
}

// stopNohup ends a relay that nohup started before.
const stopNohup = `pidfile="$HOME/.config/kou-conveyor-relay/relay.pid"
if [ -f "$pidfile" ]; then kill "$(cat "$pidfile")" 2>/dev/null || true; rm -f "$pidfile"; sleep 1; fi
`

// keepRunning has the relay run, and run again after a failure or a
// restart of the host where it can: it answers how it does.
func (in *relayInstall) keepRunning(mode, relayURL string) (string, error) {
	if mode == "systemd" {
		in.step("run it %s, kou-conveyor-relay.service", serviceSays(mode, in.root))
		unitPath, target, systemctl := `"$HOME/.config/systemd/user/kou-conveyor-relay.service"`, "default.target", "systemctl --user"
		if in.root {
			unitPath, target, systemctl = "/etc/systemd/system/kou-conveyor-relay.service", "multi-user.target", "systemctl"
		}
		script := stopNohup + fmt.Sprintf(`set -e
unit=%s
mkdir -p "$(dirname "$unit")"
cat > "$unit" <<'UNIT'
[Unit]
Description=kou-conveyor relay: carries a kou-conveyor cockpit to its clients
After=network-online.target
Wants=network-online.target

[Service]
ExecStart=%s
Restart=always
RestartSec=2

[Install]
WantedBy=%s
UNIT
%[4]s daemon-reload
%[4]s enable kou-conveyor-relay.service >/dev/null 2>&1
%[4]s restart kou-conveyor-relay.service
`, unitPath, in.relayCommand(relayURL, true), target, systemctl)
		if !in.root {
			// A user's services end with the user's last session unless
			// it lingers.
			script += `loginctl enable-linger "$(id -un)" >/dev/null 2>&1 || echo "linger: off"` + "\n"
		}
		out, err := in.remote(script, nil)
		if err == nil {
			if strings.Contains(out, "linger: off") {
				in.step("the user's services may end when the user logs out of the host: loginctl enable-linger there keeps them (as root)")
			}
			return mode, nil
		}
		if in.service != "auto" {
			return mode, err
		}
		in.step("systemd did not take it (%v); nohup instead", err)
		mode = "nohup"
	}
	in.step("run it %s; its log: ~/.config/kou-conveyor-relay/relay.log", serviceSays(mode, in.root))
	script := stopNohup + fmt.Sprintf(`set -e
dir="$HOME/.config/kou-conveyor-relay"
mkdir -p "$dir"
nohup %s >> "$dir/relay.log" 2>&1 < /dev/null &
echo $! > "$dir/relay.pid"
sleep 1
kill -0 "$(cat "$dir/relay.pid")" 2>/dev/null || { tail -n 20 "$dir/relay.log"; exit 1; }
`, in.relayCommand(relayURL, false))
	_, err := in.remote(script, nil)
	return mode, err
}

// await waits for the relay to answer from here.
func (in *relayInstall) await(config tunnel.Config) error {
	deadline := time.Now().Add(20 * time.Second)
	for {
		ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
		_, err := tunnel.Check(ctx, config)
		cancel()
		if err == nil || tunnel.Code(err) != "unreachable" || time.Now().After(deadline) {
			return err
		}
		time.Sleep(time.Second)
	}
}

// diagnose says why the relay does not answer from here: asked on its
// host, it does or not.
func (in *relayInstall) diagnose(relayURL string) string {
	scheme := "https"
	if in.tls == "off" {
		scheme = "http"
	}
	local := fmt.Sprintf("%s://127.0.0.1:%d%s", scheme, in.port, tunnel.HealthPath)
	script := fmt.Sprintf(`if command -v curl >/dev/null 2>&1; then curl -fsSk --max-time 5 %[1]s
elif command -v wget >/dev/null 2>&1; then wget -qO- --no-check-certificate -T 5 %[1]s
else echo no-client; fi`, shellQuote(local))
	out, _ := in.remote(script, nil)
	switch {
	case strings.Contains(out, `"relay":"`+tunnel.Name+`"`):
		return fmt.Sprintf("the relay runs on the host, but TCP port %d is closed between here and there: open it in the host's firewall (sudo ufw allow %d/tcp; or sudo firewall-cmd --permanent --add-port=%d/tcp && sudo firewall-cmd --reload) and in its provider's security group; or pass -url if clients reach it by another address than %s", in.port, in.port, in.port, relayURL)
	case strings.Contains(out, "no-client"):
		return fmt.Sprintf("neither curl nor wget is on the host to look from there; is the relay running (systemctl --user status kou-conveyor-relay, or ~/.config/kou-conveyor-relay/relay.log), and TCP port %d open?", in.port)
	}
	return "the relay does not answer on the host either: journalctl --user -u kou-conveyor-relay (as root: journalctl -u kou-conveyor-relay), or ~/.config/kou-conveyor-relay/relay.log, says why"
}

// relaySource is the checkout of kou-conveyor the relay is built from: the
// one this program was built from, or one the working directory is in.
func relaySource() (string, error) {
	var candidates []string
	if builtFrom != "" {
		candidates = append(candidates, builtFrom)
	}
	if wd, err := os.Getwd(); err == nil {
		for dir := wd; ; dir = filepath.Dir(dir) {
			candidates = append(candidates, dir)
			if filepath.Dir(dir) == dir {
				break
			}
		}
	}
	for _, dir := range candidates {
		if _, err := os.Stat(filepath.Join(dir, "cmd", tunnel.Name, "main.go")); err != nil {
			continue
		}
		if _, err := os.Stat(filepath.Join(dir, "go.mod")); err == nil {
			return dir, nil
		}
	}
	return "", errors.New("no checkout of kou-conveyor to build the relay from: run this inside one, or pass -binary")
}

// buildRelay builds the relay for a platform; cleanup removes the build.
func buildRelay(source, goos, goarch, goarm string) (string, func(), error) {
	gobin, err := exec.LookPath("go")
	if err != nil {
		return "", nil, errors.New("go is not on the PATH to build the relay with; pass -binary")
	}
	dir, err := os.MkdirTemp("", tunnel.Name+"-")
	if err != nil {
		return "", nil, err
	}
	cleanup := func() { os.RemoveAll(dir) }
	out := filepath.Join(dir, tunnel.Name)
	command := exec.Command(gobin, "build", "-trimpath", "-ldflags", "-s -w", "-o", out, "./cmd/"+tunnel.Name)
	command.Dir = source
	command.Env = append(os.Environ(), "CGO_ENABLED=0", "GOOS="+goos, "GOARCH="+goarch)
	if goarm != "" {
		command.Env = append(command.Env, "GOARM="+goarm)
	}
	if output, err := command.CombinedOutput(); err != nil {
		cleanup()
		return "", nil, fmt.Errorf("go build: %v\n%s", err, output)
	}
	return out, cleanup, nil
}
