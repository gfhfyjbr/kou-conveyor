package tunnel

import (
	"bytes"
	"context"
	"crypto/sha256"
	"crypto/tls"
	"encoding/hex"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"log"
	"net"
	"net/http"
	"strconv"
	"strings"
	"time"

	"github.com/gorilla/websocket"
	"github.com/hashicorp/yamux"
)

// Error is why the cockpit cannot use the relay, with a code a program
// can act on: not_configured, unreachable, not_relay, unauthorized,
// incompatible, refused.
type Error struct {
	Code    string
	Message string
	Err     error
}

func (e *Error) Error() string {
	if e.Err != nil {
		return e.Message + ": " + e.Err.Error()
	}
	return e.Message
}

func (e *Error) Unwrap() error { return e.Err }

// Code is the code of err, an *Error, or "" for any other.
func Code(err error) string {
	if e, ok := errors.AsType[*Error](err); ok {
		return e.Code
	}
	return ""
}

// TLSConfig is how the cockpit and its tools reach the relay: the usual
// verification, or, with a fingerprint, that certificate and no other.
func TLSConfig(c Config) *tls.Config {
	if c.Fingerprint == "" {
		return nil
	}
	want, err := hex.DecodeString(c.Fingerprint)
	return &tls.Config{
		MinVersion: tls.VersionTLS12,
		// The relay's own certificate names no host a CA vouched for; the
		// pin below stands for the usual verification.
		InsecureSkipVerify: true,
		VerifyConnection: func(state tls.ConnectionState) error {
			if err != nil {
				return err
			}
			if len(state.PeerCertificates) == 0 {
				return errors.New("the relay showed no certificate")
			}
			got := sha256.Sum256(state.PeerCertificates[0].Raw)
			if !bytes.Equal(got[:], want) {
				return fmt.Errorf("the relay's certificate (%x) is not the one pinned (%s)", got, c.Fingerprint)
			}
			return nil
		},
	}
}

// Check asks the relay whether it is one, takes the token, and speaks the
// cockpit's protocol.
func Check(ctx context.Context, c Config) (Health, error) {
	var health Health
	if err := c.Validate(); err != nil {
		return health, &Error{Code: "not_configured", Message: "the tunnel is not configured", Err: err}
	}
	client := &http.Client{
		Transport: &http.Transport{Proxy: http.ProxyFromEnvironment, TLSClientConfig: TLSConfig(c), DisableKeepAlives: true},
		Timeout:   8 * time.Second,
	}
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, c.Base()+HealthPath, nil)
	if err != nil {
		return health, &Error{Code: "not_configured", Message: "the relay's url", Err: err}
	}
	req.Header.Set("Authorization", "Bearer "+c.Token)
	resp, err := client.Do(req)
	if err != nil {
		return health, &Error{Code: "unreachable", Message: "cannot reach the relay at " + c.Base(), Err: err}
	}
	defer resp.Body.Close()
	data, _ := io.ReadAll(io.LimitReader(resp.Body, 64<<10))
	if resp.StatusCode != http.StatusOK || json.Unmarshal(data, &health) != nil || health.Relay != Name {
		return health, &Error{Code: "not_relay", Message: fmt.Sprintf("%s answers, but not as a kou-conveyor relay (HTTP %d)", c.Base(), resp.StatusCode)}
	}
	if !health.Authorized {
		return health, &Error{Code: "unauthorized", Message: "the relay at " + c.Base() + " does not take this token"}
	}
	if health.Protocol != Protocol {
		return health, &Error{Code: "incompatible", Message: fmt.Sprintf("the relay speaks protocol %d, this cockpit %d: update the older of the two", health.Protocol, Protocol)}
	}
	return health, nil
}

// AgentURL is where the cockpit connects: the relay's AgentPath, over ws://
// or wss://.
func AgentURL(c Config) string {
	base := c.Base()
	switch {
	case strings.HasPrefix(base, "https://"):
		base = "wss://" + strings.TrimPrefix(base, "https://")
	case strings.HasPrefix(base, "http://"):
		base = "ws://" + strings.TrimPrefix(base, "http://")
	}
	return base + AgentPath
}

// Dial connects the cockpit to the relay. The session it gives is the
// cockpit's listener: each connection a client makes to the relay comes
// out of its Accept.
func Dial(ctx context.Context, c Config) (*yamux.Session, error) {
	if err := c.Validate(); err != nil {
		return nil, &Error{Code: "not_configured", Message: "the tunnel is not configured", Err: err}
	}
	dialer := websocket.Dialer{
		Proxy:            http.ProxyFromEnvironment,
		HandshakeTimeout: 15 * time.Second,
		TLSClientConfig:  TLSConfig(c),
		ReadBufferSize:   32 << 10,
		WriteBufferSize:  32 << 10,
	}
	header := http.Header{}
	header.Set("Authorization", "Bearer "+c.Token)
	header.Set(ProtocolHeader, strconv.Itoa(Protocol))
	header.Set("User-Agent", "kou-conveyor-web")
	ws, resp, err := dialer.DialContext(ctx, AgentURL(c), header)
	if err != nil {
		return nil, dialError(c, resp, err)
	}
	session, err := yamux.Server(newConn(ws), sessionConfig())
	if err != nil {
		ws.Close()
		return nil, err
	}
	return session, nil
}

// dialError says why the relay did not take the cockpit's connection.
func dialError(c Config, resp *http.Response, err error) error {
	if resp == nil {
		return &Error{Code: "unreachable", Message: "cannot reach the relay at " + c.Base(), Err: err}
	}
	defer resp.Body.Close()
	var body struct {
		Error string `json:"error"`
		Code  string `json:"code"`
	}
	data, _ := io.ReadAll(io.LimitReader(resp.Body, 64<<10))
	if json.Unmarshal(data, &body) != nil || body.Error == "" {
		return &Error{Code: "not_relay", Message: fmt.Sprintf("%s did not take the cockpit's connection (HTTP %d); is it a kou-conveyor relay?", c.Base(), resp.StatusCode)}
	}
	code := body.Code
	if code == "" {
		code = "refused"
	}
	return &Error{Code: code, Message: "the relay refused the cockpit: " + body.Error}
}

// Serve serves handler on the session's streams until the session ends or
// ctx is done. Requests' contexts end with ctx.
func Serve(ctx context.Context, session *yamux.Session, handler http.Handler) error {
	server := &http.Server{
		Handler:           handler,
		BaseContext:       func(net.Listener) context.Context { return ctx },
		ReadHeaderTimeout: 15 * time.Second,
		IdleTimeout:       2 * time.Minute,
		// A stream the relay drops halfway is no news.
		ErrorLog: log.New(io.Discard, "", 0),
	}
	done := make(chan struct{})
	go func() {
		select {
		case <-ctx.Done():
		case <-done:
		case <-session.CloseChan():
		}
		server.Close()
		session.Close()
	}()
	err := server.Serve(session)
	close(done)
	if ctx.Err() != nil {
		return ctx.Err()
	}
	if errors.Is(err, yamux.ErrSessionShutdown) || errors.Is(err, http.ErrServerClosed) || errors.Is(err, net.ErrClosed) {
		return errors.New("the relay's connection closed")
	}
	return err
}
