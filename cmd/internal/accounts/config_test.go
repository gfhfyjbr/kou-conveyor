package accounts

import (
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func TestEnsureConfigCreates(t *testing.T) {
	dir := t.TempDir()
	path, auths := filepath.Join(dir, "config.yaml"), filepath.Join(dir, "auths")
	cfg, key, err := ensureConfig(path, auths, "127.0.0.1", 18318)
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Host != "127.0.0.1" || cfg.Port != 18318 || cfg.AuthDir != auths {
		t.Errorf("host %q port %d auth-dir %q", cfg.Host, cfg.Port, cfg.AuthDir)
	}
	if !strings.HasPrefix(key, "sk-ua-") || len(key) < 20 {
		t.Errorf("key = %q", key)
	}
	if !strings.HasPrefix(cfg.RemoteManagement.SecretKey, "$2") || cfg.RemoteManagement.AllowRemote {
		t.Errorf("management = %+v", cfg.RemoteManagement)
	}
	info, err := os.Stat(path)
	if err != nil || info.Mode().Perm() != 0o600 {
		t.Errorf("config mode = %v, %v", info.Mode(), err)
	}
	again, sameKey, err := ensureConfig(path, auths, "127.0.0.1", 18318)
	if err != nil || sameKey != key || again.RemoteManagement.SecretKey != cfg.RemoteManagement.SecretKey {
		t.Errorf("a second start keeps the key and the secret: %v", err)
	}
}

func TestEnsureConfigKeepsTheUsers(t *testing.T) {
	dir := t.TempDir()
	path := filepath.Join(dir, "config.yaml")
	original := `# my gateway
port: 9999 # an old port
auth-dir: "creds"
request-retry: 7 # mine
routing:
  strategy: fill-first
`
	if err := os.WriteFile(path, []byte(original), 0o600); err != nil {
		t.Fatal(err)
	}
	cfg, key, err := ensureConfig(path, filepath.Join(dir, "auths"), "", 18400)
	if err != nil {
		t.Fatal(err)
	}
	if cfg.Port != 18400 || cfg.Host != "" {
		t.Errorf("address = %q:%d", cfg.Host, cfg.Port)
	}
	if cfg.AuthDir != filepath.Join(dir, "creds") {
		t.Errorf("a relative auth-dir is the config's folder's: %q", cfg.AuthDir)
	}
	if cfg.RequestRetry != 7 || cfg.Routing.Strategy != "fill-first" || key == "" || cfg.RemoteManagement.SecretKey == "" {
		t.Errorf("retry %d strategy %q key %q", cfg.RequestRetry, cfg.Routing.Strategy, key)
	}
	data, _ := os.ReadFile(path)
	for _, kept := range []string{"# my gateway", "# mine", "port: 18400"} {
		if !strings.Contains(string(data), kept) {
			t.Errorf("config lost %q:\n%s", kept, data)
		}
	}
}

func TestListenAddress(t *testing.T) {
	for address, ok := range map[string]bool{
		"127.0.0.1:8318": true, ":8318": true, "0.0.0.0:8318": true, "[::1]:8318": true, "localhost:8318": true,
		"192.168.1.5:8318": false, "127.0.0.1": false, "127.0.0.1:0": false, "127.0.0.1:x": false,
	} {
		if _, _, err := listenAddress(address); (err == nil) != ok {
			t.Errorf("listenAddress(%q) = %v", address, err)
		}
	}
	if got := loopback("0.0.0.0", 1); got != "127.0.0.1:1" {
		t.Errorf("loopback = %q", got)
	}
	if got := loopback("::", 1); got != "[::1]:1" {
		t.Errorf("loopback = %q", got)
	}
}
