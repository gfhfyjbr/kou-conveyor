package accounts

import (
	"bytes"
	"crypto/rand"
	"errors"
	"fmt"
	"io/fs"
	"net"
	"os"
	"path/filepath"
	"strconv"
	"strings"

	sdkconfig "github.com/router-for-me/CLIProxyAPI/v7/sdk/config"
	"golang.org/x/crypto/bcrypt"
	"gopkg.in/yaml.v3"
)

// configTemplate is the gateway's configuration when the cockpit first
// starts it. The file is the user's to edit; the cockpit keeps only what it
// depends on in it (patchConfig).
const configTemplate = `# CLIProxyAPI, run by kou-conveyor-web as its accounts gateway.
# The cockpit keeps host, port, auth-dir, the first of api-keys and
# remote-management.secret-key as it needs them; the rest is yours to change:
# https://github.com/router-for-me/CLIProxyAPI/blob/main/config.example.yaml
host: %q
port: %d

# The signed-in accounts, one JSON credential each.
auth-dir: %q

# Keys clients present on /v1. Runs pointed at the gateway use the first.
api-keys:
  - %q

remote-management:
  # The cockpit reaches the management API on loopback, with a password of
  # its own; nothing else needs to.
  allow-remote: false
  secret-key: %q
  disable-control-panel: true
  disable-auto-update-panel: true

# A failed request is tried again up to this many times, on other accounts.
request-retry: 3
max-retry-interval: 30

routing:
  # round-robin, weighted-round-robin or fill-first
  strategy: "round-robin"

quota-exceeded:
  switch-project: true
  switch-preview-model: true

usage-statistics-enabled: true
logging-to-file: false
debug: false
`

// listenAddress splits the address the gateway listens on. Its management
// API only answers on loopback, where the cockpit reaches it, so the gateway
// listens on loopback or on every interface.
func listenAddress(address string) (host string, port int, err error) {
	host, rawPort, err := net.SplitHostPort(address)
	if err != nil {
		return "", 0, fmt.Errorf("gateway address %q: %w", address, err)
	}
	port, err = strconv.Atoi(rawPort)
	if err != nil || port <= 0 || port > 65535 {
		return "", 0, fmt.Errorf("gateway address %q: the port must be a number from 1 to 65535", address)
	}
	if host != "" && host != "localhost" {
		ip := net.ParseIP(host)
		if ip == nil || !(ip.IsLoopback() || ip.IsUnspecified()) {
			return "", 0, fmt.Errorf("gateway address %q: listen on loopback, such as 127.0.0.1:%d, or on every interface", address, port)
		}
	}
	return host, port, nil
}

// loopback is where the cockpit reaches a gateway listening on host.
func loopback(host string, port int) string {
	if host == "" || host == "localhost" {
		host = "127.0.0.1"
	} else if ip := net.ParseIP(host); ip != nil && ip.IsUnspecified() {
		host = "127.0.0.1"
		if ip.To4() == nil {
			host = "::1"
		}
	}
	return net.JoinHostPort(host, strconv.Itoa(port))
}

// ensureConfig writes the gateway's configuration on the first start and
// keeps the settings the cockpit depends on in it, then loads it. It
// returns the configuration and the API key runs present.
func ensureConfig(path, authDir, host string, port int) (*sdkconfig.Config, string, error) {
	if _, err := os.Stat(path); errors.Is(err, fs.ErrNotExist) {
		secret, err := secretHash()
		if err != nil {
			return nil, "", err
		}
		text := fmt.Sprintf(configTemplate, host, port, authDir, newAPIKey(), secret)
		if err := writeFileAtomic(path, []byte(text)); err != nil {
			return nil, "", fmt.Errorf("write the gateway configuration: %w", err)
		}
	} else if err != nil {
		return nil, "", err
	}
	if err := patchConfig(path, authDir, host, port); err != nil {
		return nil, "", err
	}
	cfg, err := sdkconfig.LoadConfigOptional(path, false)
	if err != nil {
		return nil, "", fmt.Errorf("%s: %w", path, err)
	}
	if len(cfg.APIKeys) == 0 || strings.TrimSpace(cfg.APIKeys[0]) == "" {
		return nil, "", fmt.Errorf("%s: api-keys is empty", path)
	}
	return cfg, strings.TrimSpace(cfg.APIKeys[0]), nil
}

// patchConfig brings the settings the cockpit depends on into the
// configuration, leaving everything else as the user wrote it: the address
// the server was told to listen on, an absolute credentials folder, an API
// key for runs and a management secret, without which the gateway refuses
// even the cockpit's password.
func patchConfig(path, authDir, host string, port int) error {
	data, err := os.ReadFile(path)
	if err != nil {
		return err
	}
	var doc yaml.Node
	if err := yaml.Unmarshal(data, &doc); err != nil {
		return fmt.Errorf("%s: %w", path, err)
	}
	if doc.Kind == 0 { // an empty file
		doc = yaml.Node{Kind: yaml.DocumentNode, Content: []*yaml.Node{{Kind: yaml.MappingNode, Tag: "!!map"}}}
	}
	if doc.Kind != yaml.DocumentNode || len(doc.Content) == 0 || doc.Content[0].Kind != yaml.MappingNode {
		return fmt.Errorf("%s: expected a mapping of settings", path)
	}
	root := doc.Content[0]
	changed := false
	set := func(node *yaml.Node, key, tag, value string) {
		v := mappingValue(node, key, true)
		if v.Kind != yaml.ScalarNode || v.Value != value || v.Tag != tag {
			*v = yaml.Node{Kind: yaml.ScalarNode, Tag: tag, Value: value, Style: v.Style & yaml.DoubleQuotedStyle, LineComment: v.LineComment, HeadComment: v.HeadComment}
			if tag != "!!str" {
				v.Style = 0
			}
			changed = true
		}
	}
	current := func(node *yaml.Node, key string) string {
		if v := mappingValue(node, key, false); v != nil && v.Kind == yaml.ScalarNode {
			return strings.TrimSpace(v.Value)
		}
		return ""
	}

	set(root, "host", "!!str", host)
	set(root, "port", "!!int", strconv.Itoa(port))
	// A relative folder would be the server's working directory's; the
	// gateway expands ~ itself.
	switch dir := current(root, "auth-dir"); {
	case dir == "":
		set(root, "auth-dir", "!!str", authDir)
	case !filepath.IsAbs(dir) && !strings.HasPrefix(dir, "~"):
		set(root, "auth-dir", "!!str", filepath.Join(filepath.Dir(path), dir))
	}
	keys := mappingValue(root, "api-keys", true)
	if keys.Kind != yaml.SequenceNode || len(keys.Content) == 0 || strings.TrimSpace(keys.Content[0].Value) == "" {
		*keys = yaml.Node{Kind: yaml.SequenceNode, Tag: "!!seq", Content: []*yaml.Node{
			{Kind: yaml.ScalarNode, Tag: "!!str", Value: newAPIKey(), Style: yaml.DoubleQuotedStyle},
		}}
		changed = true
	}
	management := mappingValue(root, "remote-management", true)
	if management.Kind != yaml.MappingNode {
		*management = yaml.Node{Kind: yaml.MappingNode, Tag: "!!map"}
		changed = true
	}
	if current(management, "secret-key") == "" {
		secret, err := secretHash()
		if err != nil {
			return err
		}
		set(management, "secret-key", "!!str", secret)
	}
	if !changed {
		return nil
	}
	var out bytes.Buffer
	encoder := yaml.NewEncoder(&out)
	encoder.SetIndent(2)
	if err := encoder.Encode(&doc); err != nil {
		return fmt.Errorf("%s: %w", path, err)
	}
	if err := encoder.Close(); err != nil {
		return err
	}
	return writeFileAtomic(path, out.Bytes())
}

// mappingValue finds the value of key in a mapping, adding an empty one at
// the end when create is set.
func mappingValue(mapping *yaml.Node, key string, create bool) *yaml.Node {
	for i := 0; i+1 < len(mapping.Content); i += 2 {
		if mapping.Content[i].Value == key {
			return mapping.Content[i+1]
		}
	}
	if !create {
		return nil
	}
	value := &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!null"}
	mapping.Content = append(mapping.Content, &yaml.Node{Kind: yaml.ScalarNode, Tag: "!!str", Value: key}, value)
	return value
}

// newAPIKey makes a key for clients of the gateway.
func newAPIKey() string { return "sk-ua-" + strings.ToLower(rand.Text()) }

// secretHash is the hash of a management secret nobody knows. The gateway
// turns management away without a secret, even the cockpit with its own
// password; the cockpit never needs the secret itself.
func secretHash() (string, error) {
	hash, err := bcrypt.GenerateFromPassword([]byte(rand.Text()+rand.Text()), bcrypt.DefaultCost)
	if err != nil {
		return "", err
	}
	return string(hash), nil
}

// writeFileAtomic replaces a file, readable only by the user, so that a
// reader never sees half of it.
func writeFileAtomic(path string, data []byte) error {
	dir := filepath.Dir(path)
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return err
	}
	file, err := os.CreateTemp(dir, "."+filepath.Base(path)+"-*")
	if err != nil {
		return err
	}
	defer os.Remove(file.Name())
	if err := file.Chmod(0o600); err != nil {
		file.Close()
		return err
	}
	_, err = file.Write(data)
	if err == nil {
		err = file.Sync()
	}
	if closeErr := file.Close(); err == nil {
		err = closeErr
	}
	if err != nil {
		return err
	}
	return os.Rename(file.Name(), path)
}
