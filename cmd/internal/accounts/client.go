package accounts

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"strings"
	"time"
)

// Client speaks the gateway's management API, as the cockpit does: on
// loopback, with the password the gateway was started with.
type Client struct {
	base     string // http://127.0.0.1:8318
	password string
	http     *http.Client
}

func newClient(base, password string) *Client {
	return &Client{base: strings.TrimRight(base, "/"), password: password, http: &http.Client{Timeout: 90 * time.Second}}
}

// APIError is the gateway's answer to a request it refused.
type APIError struct {
	Status  int
	Message string
}

func (e *APIError) Error() string {
	if e.Message == "" {
		return fmt.Sprintf("the gateway answered %d %s", e.Status, http.StatusText(e.Status))
	}
	return e.Message
}

// ErrUnreachable is the error of a request the gateway did not answer.
var ErrUnreachable = errors.New("the gateway is unreachable")

// StatusOf is the HTTP status of the gateway's refusal, or 0.
func StatusOf(err error) int {
	var apiErr *APIError
	if errors.As(err, &apiErr) {
		return apiErr.Status
	}
	return 0
}

// do sends a management request. A body that is not a reader is sent as
// JSON; out receives a JSON answer.
func (c *Client) do(ctx context.Context, method, path string, body any, out any) error {
	var reader io.Reader
	contentType := ""
	switch b := body.(type) {
	case nil:
	case []byte:
		reader, contentType = bytes.NewReader(b), "application/json"
	default:
		data, err := json.Marshal(b)
		if err != nil {
			return err
		}
		reader, contentType = bytes.NewReader(data), "application/json"
	}
	req, err := http.NewRequestWithContext(ctx, method, c.base+path, reader)
	if err != nil {
		return err
	}
	req.Header.Set("Authorization", "Bearer "+c.password)
	req.Header.Set("Accept", "application/json")
	if contentType != "" {
		req.Header.Set("Content-Type", contentType)
	}
	res, err := c.http.Do(req)
	if err != nil {
		return fmt.Errorf("%w: %w", ErrUnreachable, err)
	}
	defer res.Body.Close()
	data, err := io.ReadAll(io.LimitReader(res.Body, 16<<20))
	if err != nil {
		return fmt.Errorf("read the gateway's answer: %w", err)
	}
	if res.StatusCode < 200 || res.StatusCode > 299 {
		var refusal struct {
			Error   string `json:"error"`
			Message string `json:"message"`
		}
		_ = json.Unmarshal(data, &refusal)
		return &APIError{Status: res.StatusCode, Message: firstNonEmpty(refusal.Error, refusal.Message)}
	}
	if out == nil {
		return nil
	}
	if raw, ok := out.(*[]byte); ok {
		*raw = data
		return nil
	}
	if err := json.Unmarshal(data, out); err != nil {
		return fmt.Errorf("the gateway's answer to %s %s: %w", method, path, err)
	}
	return nil
}

// ready reports whether the management API answers.
func (c *Client) ready(ctx context.Context) error {
	return c.do(ctx, http.MethodGet, "/v0/management/config", nil, nil)
}

// authFiles lists the gateway's accounts: those with a credential file and
// those its configuration defines. Entries are kept as the gateway sends
// them, since what an entry holds depends on its provider.
func (c *Client) authFiles(ctx context.Context) ([]record, error) {
	var list struct {
		Files []map[string]any `json:"files"`
	}
	if err := c.do(ctx, http.MethodGet, "/v0/management/auth-files", nil, &list); err != nil {
		return nil, err
	}
	out := make([]record, 0, len(list.Files))
	for _, file := range list.Files {
		out = append(out, record(file))
	}
	return out, nil
}

// Login is a sign-in in progress.
type Login struct {
	State    string `json:"state"`
	Provider string `json:"provider"`
	// URL is the page the user signs in on.
	URL  string `json:"url"`
	Flow string `json:"flow"`
	// UserCode is the code a device sign-in asks for on that page.
	UserCode  string    `json:"user_code,omitzero"`
	ExpiresAt time.Time `json:"expires_at,omitzero"`
	// Manual is set when no callback listens on this machine: the user
	// brings the address the provider sends the browser to (FinishLogin).
	Manual bool `json:"manual,omitzero"`
}

// startLogin asks the gateway to start a sign-in. With a callback, the
// gateway listens on the port the provider sends the browser back to.
func (c *Client) startLogin(ctx context.Context, p Provider, callback bool) (Login, error) {
	var answer struct {
		URL       string `json:"url"`
		State     string `json:"state"`
		Flow      string `json:"flow"`
		UserCode  string `json:"user_code"`
		ExpiresIn int64  `json:"expires_in"`
	}
	path := "/v0/management/" + p.endpoint
	if callback {
		path += "?is_webui=true"
	}
	if err := c.do(ctx, http.MethodGet, path, nil, &answer); err != nil {
		return Login{}, err
	}
	if answer.State == "" || answer.URL == "" {
		return Login{}, errors.New("the gateway did not start the sign-in")
	}
	login := Login{State: answer.State, Provider: p.ID, URL: answer.URL, Flow: p.Flow, UserCode: answer.UserCode}
	if answer.Flow == FlowDevice {
		login.Flow = FlowDevice
	}
	if answer.ExpiresIn > 0 {
		login.ExpiresAt = time.Now().Add(time.Duration(answer.ExpiresIn) * time.Second).UTC()
	}
	login.Manual = login.Flow == FlowRedirect && !callback
	return login, nil
}

// LoginState is how a sign-in stands: "wait", "ok" or "error".
type LoginState struct {
	Status string `json:"status"`
	Error  string `json:"error,omitzero"`
}

func (c *Client) loginState(ctx context.Context, state string) (LoginState, error) {
	var s LoginState
	err := c.do(ctx, http.MethodGet, "/v0/management/get-auth-status?state="+url.QueryEscape(state), nil, &s)
	if s.Status == "" && err == nil {
		s.Status = "wait"
	}
	return s, err
}

// finishLogin hands the gateway the address the provider sent the browser
// to, for a sign-in whose callback could not reach this machine.
func (c *Client) finishLogin(ctx context.Context, state, redirect string) error {
	return c.do(ctx, http.MethodPost, "/v0/management/oauth-callback", map[string]string{"state": state, "redirect_url": redirect}, nil)
}

func (c *Client) cancelLogin(ctx context.Context, state string) error {
	return c.do(ctx, http.MethodDelete, "/v0/management/oauth-session?state="+url.QueryEscape(state), nil, nil)
}

func (c *Client) setDisabled(ctx context.Context, name string, disabled bool) error {
	return c.do(ctx, http.MethodPatch, "/v0/management/auth-files/status", map[string]any{"name": name, "disabled": disabled}, nil)
}

func (c *Client) remove(ctx context.Context, name string) error {
	return c.do(ctx, http.MethodDelete, "/v0/management/auth-files?name="+url.QueryEscape(name), nil, nil)
}

func (c *Client) upload(ctx context.Context, name string, data []byte) error {
	return c.do(ctx, http.MethodPost, "/v0/management/auth-files?name="+url.QueryEscape(name), data, nil)
}

// refresh has the gateway refresh an account's token now.
func (c *Client) refresh(ctx context.Context, name string) error {
	return c.do(ctx, http.MethodPost, "/v0/management/auth-files/refresh", map[string]string{"name": name}, nil)
}

// download reads an account's credential file. It holds tokens, so it is
// for the cockpit's own use and never leaves the server.
func (c *Client) download(ctx context.Context, name string) (map[string]any, error) {
	var raw []byte
	if err := c.do(ctx, http.MethodGet, "/v0/management/auth-files/download?name="+url.QueryEscape(name), nil, &raw); err != nil {
		return nil, err
	}
	var file map[string]any
	if err := json.Unmarshal(raw, &file); err != nil {
		return nil, err
	}
	return file, nil
}

// apiCall is a request the gateway sends to a provider for an account,
// with $TOKEN$ standing for the account's access token.
type apiCall struct {
	AuthIndex string            `json:"auth_index"`
	Method    string            `json:"method"`
	URL       string            `json:"url"`
	Header    map[string]string `json:"header,omitzero"`
	Data      string            `json:"data,omitzero"`
}

type apiAnswer struct {
	StatusCode int                 `json:"status_code"`
	Header     map[string][]string `json:"header"`
	Body       string              `json:"body"`
}

func (c *Client) call(ctx context.Context, request apiCall) (apiAnswer, error) {
	var answer apiAnswer
	err := c.do(ctx, http.MethodPost, "/v0/management/api-call", request, &answer)
	return answer, err
}

// record is one entry of the gateway's account list.
type record map[string]any

func (r record) str(key string) string {
	switch v := r[key].(type) {
	case string:
		return strings.TrimSpace(v)
	case float64:
		return strings.TrimSuffix(fmt.Sprintf("%f", v), ".000000")
	}
	return ""
}

func (r record) num(key string) float64 {
	switch v := r[key].(type) {
	case float64:
		return v
	case string:
		var f float64
		if _, err := fmt.Sscan(v, &f); err == nil {
			return f
		}
	}
	return 0
}

func (r record) flag(key string) bool {
	v, _ := r[key].(bool)
	return v
}

func (r record) when(key string) time.Time {
	return parseTime(r[key])
}

func (r record) obj(key string) record {
	v, _ := r[key].(map[string]any)
	return record(v)
}

func (r record) list(key string) []any {
	v, _ := r[key].([]any)
	return v
}

// parseTime reads the times providers and the gateway write: RFC 3339,
// with or without fractions, or seconds or milliseconds since the epoch.
func parseTime(v any) time.Time {
	switch v := v.(type) {
	case string:
		v = strings.TrimSpace(v)
		if v == "" || strings.HasPrefix(v, "0001-01-01") {
			return time.Time{}
		}
		for _, layout := range []string{time.RFC3339Nano, "2006-01-02T15:04:05.999999999", "2006-01-02 15:04:05"} {
			if t, err := time.Parse(layout, v); err == nil {
				return t.UTC()
			}
		}
		var n float64
		if _, err := fmt.Sscan(v, &n); err == nil {
			return parseTime(n)
		}
	case float64:
		switch {
		case v <= 0:
			return time.Time{}
		case v > 1e12: // milliseconds
			return time.UnixMilli(int64(v)).UTC()
		default:
			return time.Unix(int64(v), 0).UTC()
		}
	}
	return time.Time{}
}
