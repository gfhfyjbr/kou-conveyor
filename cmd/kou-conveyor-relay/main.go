// Command kou-conveyor-relay runs on a host the phone and the browsers
// elsewhere reach, and carries a kou-conveyor-web cockpit to them: the
// cockpit connects out to it (kou-conveyor-web tunnel), and each request a
// client with the relay's token makes goes down that connection. It keeps
// nothing but its token and its certificate.
package main

import (
	"context"
	"crypto/tls"
	"encoding/json/v2"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"log"
	"net"
	"net/http"
	"os"
	"os/signal"
	"path/filepath"
	"runtime/debug"
	"strconv"
	"strings"
	"syscall"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/tunnel"
)

// version is set by the build (-X main.version=…); else the module's.
var version string

type options struct {
	listen, dir, tlsMode, certFile, keyFile string
	names                                   []string
	printConfig, printVersion               bool
}

func parse(args []string, output io.Writer) (options, error) {
	var o options
	var names string
	f := flag.NewFlagSet(tunnel.Name, flag.ContinueOnError)
	f.SetOutput(output)
	f.StringVar(&o.listen, "listen", envOr("KOU_RELAY_LISTEN", ":"+strconv.Itoa(tunnel.DefaultPort)), "address to listen on (also KOU_RELAY_LISTEN)")
	f.StringVar(&o.dir, "dir", envOr("KOU_RELAY_DIR", ""), "where the token and the certificate are kept (default: the user config directory's kou-conveyor-relay; also KOU_RELAY_DIR)")
	f.StringVar(&o.tlsMode, "tls", envOr("KOU_RELAY_TLS", "self-signed"), "self-signed (a certificate of its own, which clients pin), files (-tls-cert and -tls-key), or off (plain HTTP: behind a TLS proxy or on a private network such as Tailscale)")
	f.StringVar(&o.certFile, "tls-cert", "", "certificate file (PEM) for -tls files")
	f.StringVar(&o.keyFile, "tls-key", "", "key file (PEM) for -tls files")
	f.StringVar(&names, "names", "", "comma-separated host names and addresses the self-signed certificate is made for")
	f.BoolVar(&o.printConfig, "print-config", false, "make the token and the certificate when there are none, print what a cockpit needs as JSON, and exit")
	f.BoolVar(&o.printVersion, "version", false, "print the version and exit")
	if err := f.Parse(args); err != nil {
		return o, err
	}
	if f.NArg() != 0 {
		return o, errors.New("unexpected arguments")
	}
	for name := range strings.SplitSeq(names, ",") {
		if name = strings.TrimSpace(name); name != "" {
			o.names = append(o.names, name)
		}
	}
	if o.certFile != "" || o.keyFile != "" {
		o.tlsMode = "files"
	}
	switch o.tlsMode {
	case "self-signed", "off":
	case "files":
		if o.certFile == "" || o.keyFile == "" {
			return o, errors.New("-tls files needs -tls-cert and -tls-key")
		}
	default:
		return o, fmt.Errorf("-tls must be self-signed, files or off, not %q", o.tlsMode)
	}
	if o.dir == "" {
		base, err := os.UserConfigDir()
		if err != nil {
			return o, fmt.Errorf("no directory for the relay's files: %w; pass -dir", err)
		}
		o.dir = filepath.Join(base, tunnel.Name)
	}
	return o, nil
}

func main() { os.Exit(run(os.Args[1:])) }

func run(args []string) int {
	o, err := parse(args, os.Stderr)
	if errors.Is(err, flag.ErrHelp) {
		return 0
	}
	if err != nil {
		fmt.Fprintln(os.Stderr, tunnel.Name+":", err)
		return 2
	}
	if o.printVersion {
		fmt.Println(tunnel.Name, currentVersion(), "protocol", tunnel.Protocol)
		return 0
	}
	token, err := loadToken(o.dir)
	if err != nil {
		fmt.Fprintln(os.Stderr, tunnel.Name+":", err)
		return 1
	}
	var certificate *tls.Certificate
	fingerprint := ""
	switch o.tlsMode {
	case "self-signed":
		cert, fp, err := tunnel.SelfSigned(o.dir, o.names)
		if err != nil {
			fmt.Fprintln(os.Stderr, tunnel.Name+":", err)
			return 1
		}
		certificate, fingerprint = &cert, fp
	case "files":
		cert, err := tls.LoadX509KeyPair(o.certFile, o.keyFile)
		if err != nil {
			fmt.Fprintln(os.Stderr, tunnel.Name+":", err)
			return 1
		}
		certificate = &cert
	}
	if o.printConfig {
		data, _ := json.Marshal(map[string]any{
			"relay": tunnel.Name, "version": currentVersion(), "protocol": tunnel.Protocol,
			"listen": o.listen, "tls": o.tlsMode, "token": token, "fingerprint": fingerprint,
		})
		fmt.Println(string(data))
		return 0
	}

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt, syscall.SIGTERM)
	defer stop()
	listener, err := net.Listen("tcp", o.listen)
	if err != nil {
		fmt.Fprintln(os.Stderr, tunnel.Name+":", err)
		return 1
	}
	logger := log.New(os.Stdout, "", log.LstdFlags)
	relay := tunnel.NewRelay(token, currentVersion(), logger.Printf)
	server := &http.Server{
		Handler:           relay,
		ReadHeaderTimeout: 15 * time.Second,
		IdleTimeout:       2 * time.Minute,
		ErrorLog:          log.New(io.Discard, "", 0),
	}
	scheme := "http"
	if certificate != nil {
		scheme = "https"
		server.TLSConfig = &tls.Config{Certificates: []tls.Certificate{*certificate}, MinVersion: tls.VersionTLS12}
		listener = tls.NewListener(listener, server.TLSConfig)
	}
	served := make(chan error, 1)
	go func() { served <- server.Serve(listener) }()
	logger.Printf("%s %s listening on %s://%s", tunnel.Name, currentVersion(), scheme, listener.Addr())
	if fingerprint != "" {
		logger.Printf("certificate fingerprint (sha-256) %s", fingerprint)
	}
	if o.tlsMode == "off" {
		logger.Printf("warning: plain HTTP; the token crosses the network as it is: use it behind TLS or on a private network")
	}
	select {
	case err := <-served:
		fmt.Fprintln(os.Stderr, tunnel.Name+":", err)
		return 1
	case <-ctx.Done():
	}
	shutdown, cancel := context.WithTimeout(context.Background(), 5*time.Second)
	defer cancel()
	relay.Close()
	_ = server.Shutdown(shutdown)
	return 0
}

// loadToken reads the relay's token: KOU_RELAY_TOKEN, else the token file
// in dir, which is made with a new token the first time.
func loadToken(dir string) (string, error) {
	if token := strings.TrimSpace(os.Getenv("KOU_RELAY_TOKEN")); token != "" {
		if len(token) < 16 {
			return "", errors.New("KOU_RELAY_TOKEN must be 16 characters at least")
		}
		return token, nil
	}
	path := filepath.Join(dir, "token")
	data, err := os.ReadFile(path)
	if err == nil {
		if token := strings.TrimSpace(string(data)); len(token) >= 16 {
			return token, nil
		}
		return "", fmt.Errorf("%s holds no token of 16 characters at least", path)
	}
	if !errors.Is(err, fs.ErrNotExist) {
		return "", err
	}
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return "", err
	}
	token := tunnel.NewToken()
	if err := os.WriteFile(path, []byte(token+"\n"), 0o600); err != nil {
		return "", err
	}
	return token, nil
}

func currentVersion() string {
	if version != "" {
		return version
	}
	if info, ok := debug.ReadBuildInfo(); ok {
		if v := info.Main.Version; v != "" && v != "(devel)" {
			return v
		}
		for _, setting := range info.Settings {
			if setting.Key == "vcs.revision" && len(setting.Value) >= 12 {
				return "devel-" + setting.Value[:12]
			}
		}
	}
	return "devel"
}

func envOr(name, fallback string) string {
	if value := strings.TrimSpace(os.Getenv(name)); value != "" {
		return value
	}
	return fallback
}
