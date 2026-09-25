package openaicodex

import (
	"encoding/base64"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"strings"
	"time"
)

type credentials struct {
	accessToken string
	accountID   string
}

// EnvironmentConfig selects a source without reading files or consulting API keys.
func EnvironmentConfig(getenv func(string) string) (Config, error) {
	config := Config{
		AccessToken: strings.TrimSpace(getenv("OPENAI_CODEX_ACCESS_TOKEN")),
		AccountID:   strings.TrimSpace(getenv("OPENAI_CODEX_ACCOUNT_ID")),
		AuthFile:    strings.TrimSpace(getenv("OPENAI_CODEX_AUTH_FILE")),
	}
	if config.AuthFile != "" && (config.AccessToken != "" || config.AccountID != "") {
		return Config{}, errors.New("choose OPENAI_CODEX_AUTH_FILE or OPENAI_CODEX_ACCESS_TOKEN/OPENAI_CODEX_ACCOUNT_ID, not both")
	}
	if config.AccessToken != "" || config.AccountID != "" || config.AuthFile != "" {
		return config, nil
	}
	home := strings.TrimSpace(getenv("CODEX_HOME"))
	if home == "" {
		userHome := strings.TrimSpace(getenv("HOME"))
		if userHome == "" {
			var err error
			userHome, err = os.UserHomeDir()
			if err != nil {
				return Config{}, fmt.Errorf("find Codex home: %w", err)
			}
		}
		home = filepath.Join(userHome, ".codex")
	}
	config.AuthFile = filepath.Join(home, "auth.json")
	return config, nil
}

func (config Config) credentials() (credentials, error) {
	token, accountID := strings.TrimSpace(config.AccessToken), strings.TrimSpace(config.AccountID)
	if config.AuthFile != "" {
		if token != "" || accountID != "" {
			return credentials{}, errors.New("codex AuthFile cannot be combined with AccessToken or AccountID")
		}
		var err error
		token, accountID, err = readAuthFile(config.AuthFile)
		if err != nil {
			return credentials{}, err
		}
	}
	if token == "" {
		return credentials{}, errors.New("codex access token must be set; use OPENAI_CODEX_ACCESS_TOKEN or a ChatGPT-authenticated Codex auth file")
	}
	if strings.HasPrefix(token, "sk-") || !headerValue(token) {
		return credentials{}, errors.New("codex requires a subscription access token, not an API key or invalid header value")
	}
	claimAccount, expires, err := tokenClaims(token)
	if err != nil {
		return credentials{}, err
	}
	if expires != 0 && time.Now().Unix() >= expires {
		return credentials{}, errors.New("codex access token has expired; renew credentials externally (interactive login and token refresh are not implemented)")
	}
	if accountID == "" {
		accountID = claimAccount
	} else if claimAccount != "" && accountID != claimAccount {
		return credentials{}, errors.New("codex account ID does not match the access token")
	}
	if accountID == "" || !headerValue(accountID) {
		return credentials{}, errors.New("codex account ID must be set in OPENAI_CODEX_ACCOUNT_ID, the auth file, or the access token")
	}
	return credentials{accessToken: token, accountID: accountID}, nil
}

func readAuthFile(path string) (string, string, error) {
	file, err := os.Open(path)
	if err != nil {
		return "", "", fmt.Errorf("open Codex auth file (provide existing ChatGPT credentials; login is not implemented): %w", err)
	}
	defer func() { _ = file.Close() }()
	info, err := file.Stat()
	if err != nil {
		return "", "", fmt.Errorf("inspect Codex auth file: %w", err)
	}
	if !info.Mode().IsRegular() || info.Mode().Perm()&0o077 != 0 {
		return "", "", errors.New("codex auth file must be a regular file with private permissions (chmod 600)")
	}
	const limit = 1 << 20
	data, err := io.ReadAll(io.LimitReader(file, limit+1))
	if err != nil {
		return "", "", fmt.Errorf("read Codex auth file: %w", err)
	}
	var auth struct {
		Mode   string `json:"auth_mode"`
		Tokens struct {
			AccessToken string `json:"access_token"`
			AccountID   string `json:"account_id"`
		} `json:"tokens"`
	}
	if len(data) > limit || json.Unmarshal(data, &auth) != nil {
		return "", "", errors.New("invalid Codex auth file; expected a JSON object with tokens.access_token and tokens.account_id")
	}
	if auth.Mode != "" && auth.Mode != "chatgpt" {
		return "", "", errors.New("codex auth file is not a ChatGPT subscription login")
	}
	return strings.TrimSpace(auth.Tokens.AccessToken), strings.TrimSpace(auth.Tokens.AccountID), nil
}

// Unverified claims are routing/expiry hints; the server authenticates the token.
func tokenClaims(token string) (string, int64, error) {
	parts := strings.Split(token, ".")
	if len(parts) != 3 {
		return "", 0, nil
	}
	payload, err := base64.RawURLEncoding.DecodeString(parts[1])
	if err != nil {
		return "", 0, errors.New("invalid Codex access token claims")
	}
	var claims struct {
		Expires int64 `json:"exp"`
		Auth    struct {
			AccountID string `json:"chatgpt_account_id"`
		} `json:"https://api.openai.com/auth"`
	}
	if json.Unmarshal(payload, &claims) != nil {
		return "", 0, errors.New("invalid Codex access token claims")
	}
	return claims.Auth.AccountID, claims.Expires, nil
}

func headerValue(value string) bool {
	for _, char := range value {
		if char <= ' ' || char > '~' {
			return false
		}
	}
	return value != ""
}
