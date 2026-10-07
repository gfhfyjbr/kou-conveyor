// Package tunnel carries a kou-conveyor-web cockpit through a relay: a small
// server on a host that phones and browsers elsewhere reach. The cockpit
// dials the relay — a WebSocket out, so nothing new listens on the machine
// it runs on — and the relay hands every connection a client makes to it
// down that one connection, multiplexed (yamux). The cockpit serves each as
// if it had been made to it: the sessions, the run events, the canvases, the
// terminals' WebSockets and all. The relay keeps nothing; it checks the
// token and passes the bytes.
package tunnel

import (
	"crypto/rand"
	"encoding/base64"
	"encoding/hex"
	"encoding/json/v2"
	"errors"
	"fmt"
	"net/url"
	"os"
	"path/filepath"
	"strings"
)

// Protocol is the version of what the relay and the cockpit say to each
// other; the two must agree on it.
const Protocol = 1

const (
	// Name is what the relay calls itself.
	Name = "kou-conveyor-relay"
	// AgentPath is where the cockpit connects to the relay.
	AgentPath = "/_relay/agent"
	// HealthPath says what the relay is, and, to the token, whether a
	// cockpit is connected.
	HealthPath = "/_relay/health"
	// ProtocolHeader carries Protocol on the cockpit's connection.
	ProtocolHeader = "X-Kou-Relay-Protocol"
	// TokenCookie keeps a browser's token; TokenQuery brings it once.
	TokenCookie = "kou_relay"
	TokenQuery  = "kou_token"
	// DefaultPort is the relay's port unless it is told another.
	DefaultPort = 8420
	// PairScheme is the URL scheme of the links that pair the iPhone app.
	PairScheme = "kouconveyor"
)

// Config says where the relay is and how to reach it: what the cockpit
// keeps in tunnel.json.
type Config struct {
	// URL is the relay's base URL: http(s)://host:port.
	URL string `json:"url"`
	// Token is the relay's secret, which the cockpit and its clients send.
	Token string `json:"token"`
	// Fingerprint pins the relay's own certificate: the SHA-256 of its DER,
	// in hex. Without one the certificate is verified as any is.
	Fingerprint string `json:"fingerprint,omitempty"`
	// Host is the SSH destination the relay was installed on, if it was.
	Host string `json:"host,omitempty"`
	// Enabled has the cockpit connect whenever it runs: set once a tunnel
	// came up, cleared when it is stopped.
	Enabled bool `json:"enabled,omitempty"`
}

// Validate reports what makes the configuration unusable.
func (c Config) Validate() error {
	u, err := url.Parse(c.URL)
	switch {
	case c.URL == "":
		return errors.New("the relay's url is missing")
	case err != nil:
		return fmt.Errorf("the relay's url: %w", err)
	case u.Scheme != "http" && u.Scheme != "https":
		return fmt.Errorf("the relay's url must be http:// or https://, not %q", c.URL)
	case u.Host == "":
		return fmt.Errorf("the relay's url has no host: %q", c.URL)
	case u.Path != "" && u.Path != "/" || u.RawQuery != "" || u.Fragment != "":
		return fmt.Errorf("the relay's url must be the bare http(s)://host:port, not %q", c.URL)
	case len(c.Token) < 16:
		return errors.New("the relay's token is missing or too short (16 characters at least)")
	case strings.ContainsAny(c.Token, " \t\r\n;,\""):
		return errors.New("the relay's token holds characters a header or a cookie cannot carry")
	}
	if c.Fingerprint != "" {
		if _, err := NormalizeFingerprint(c.Fingerprint); err != nil {
			return err
		}
	}
	return nil
}

// Base is the relay's URL without a trailing slash.
func (c Config) Base() string { return strings.TrimRight(c.URL, "/") }

// LoadConfig reads the configuration at path. A missing file is an error
// that wraps fs.ErrNotExist; a file that says something unusable is one too.
func LoadConfig(path string) (Config, error) {
	var c Config
	data, err := os.ReadFile(path)
	if err != nil {
		return c, err
	}
	if err := json.Unmarshal(data, &c); err != nil {
		return c, fmt.Errorf("%s: %w", path, err)
	}
	if c.Fingerprint != "" {
		c.Fingerprint, _ = NormalizeFingerprint(c.Fingerprint)
	}
	if err := c.Validate(); err != nil {
		return c, fmt.Errorf("%s: %w", path, err)
	}
	return c, nil
}

// SaveConfig writes the configuration to path, readable by its owner only:
// it holds the token. The file is replaced whole.
func SaveConfig(path string, c Config) error {
	if err := c.Validate(); err != nil {
		return err
	}
	data, err := json.Marshal(c, json.Deterministic(true))
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return err
	}
	file, err := os.CreateTemp(filepath.Dir(path), ".tunnel-*.json")
	if err != nil {
		return err
	}
	defer os.Remove(file.Name())
	if _, err := file.Write(append(data, '\n')); err != nil {
		file.Close()
		return err
	}
	if err := file.Chmod(0o600); err != nil {
		file.Close()
		return err
	}
	if err := file.Close(); err != nil {
		return err
	}
	return os.Rename(file.Name(), path)
}

// NewToken makes a relay token: 256 random bits, URL-safe.
func NewToken() string {
	b := make([]byte, 32)
	rand.Read(b)
	return base64.RawURLEncoding.EncodeToString(b)
}

// NormalizeFingerprint takes a SHA-256 fingerprint as it may be written —
// upper case, colons, spaces — and gives it as 64 lower-case hex digits.
func NormalizeFingerprint(s string) (string, error) {
	s = strings.ToLower(strings.NewReplacer(":", "", " ", "", "\n", "", "\t", "").Replace(strings.TrimSpace(s)))
	if b, err := hex.DecodeString(s); err != nil || len(b) != 32 {
		return "", fmt.Errorf("the fingerprint must be a SHA-256 in hex (64 digits), not %q", s)
	}
	return s, nil
}

// PairLink is the link that pairs the iPhone app with the relay: opened on
// the phone, or scanned as a QR code, it has the app connect.
func (c Config) PairLink() string {
	q := url.Values{}
	q.Set("url", c.Base())
	q.Set("token", c.Token)
	if c.Fingerprint != "" {
		q.Set("fp", c.Fingerprint)
	}
	return PairScheme + "://pair?" + q.Encode()
}

// BrowserLink opens the cockpit through the relay in a browser: the relay
// keeps the token in a cookie and drops it from the address.
func (c Config) BrowserLink() string {
	return c.Base() + "/?" + TokenQuery + "=" + url.QueryEscape(c.Token)
}
