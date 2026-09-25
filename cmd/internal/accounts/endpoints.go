package accounts

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"net/url"
	"regexp"
	"slices"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm/clients/anthropic"
)

// EndpointKind is a kind of upstream the gateway reaches with an API key,
// beside the accounts signed in with OAuth. Each is a list in the gateway's
// configuration, which its management API edits.
type EndpointKind struct {
	ID     string `json:"id"`
	Name   string `json:"name"`
	Detail string `json:"detail"`
	// BaseURL is the kind's usual address, used when an endpoint gives none.
	BaseURL string `json:"base_url,omitzero"`
	// Named endpoints have a name, which tells them apart; it must be unique.
	Named bool `json:"named,omitzero"`
	// NeedsBase: the kind has no usual address.
	NeedsBase bool `json:"needs_base,omitzero"`
	// NeedsModels: the gateway serves only the models listed; a kind that
	// does not need them serves its provider's known models without a list.
	NeedsModels bool `json:"needs_models,omitzero"`
	// section is the configuration's list, and its management route.
	section string
	// probe is how the upstream lists its models: "openai", "anthropic" or
	// "gemini".
	probe string
	// fillBase: the gateway drops an endpoint of the kind without a base URL,
	// so the usual one is written out.
	fillBase bool
}

var endpointKinds = []EndpointKind{
	{ID: "openai-compatible", Name: "OpenAI-compatible", Detail: "Chat Completions API: OpenRouter, DeepSeek, Groq, Together, Ollama, vLLM, LM Studio and others",
		Named: true, NeedsBase: true, NeedsModels: true, section: "openai-compatibility", probe: "openai"},
	{ID: "anthropic", Name: "Anthropic API", Detail: "Messages API with an Anthropic key, or another gateway that speaks it",
		BaseURL: "https://api.anthropic.com", section: "claude-api-key", probe: "anthropic"},
	{ID: "openai", Name: "OpenAI API", Detail: "Responses API with an OpenAI key, or another gateway that speaks it",
		BaseURL: "https://api.openai.com/v1", NeedsModels: true, section: "codex-api-key", probe: "openai", fillBase: true},
	{ID: "gemini", Name: "Gemini API", Detail: "Gemini API key from Google AI Studio",
		BaseURL: "https://generativelanguage.googleapis.com", section: "gemini-api-key", probe: "gemini"},
	{ID: "xai", Name: "xAI API", Detail: "xAI API key for Grok models",
		BaseURL: "https://api.x.ai/v1", section: "xai-api-key", probe: "openai", fillBase: true},
}

// EndpointKinds lists the kinds of endpoint the gateway takes.
func EndpointKinds() []EndpointKind { return slices.Clone(endpointKinds) }

func endpointKind(id string) (EndpointKind, bool) {
	for _, k := range endpointKinds {
		if k.ID == strings.ToLower(strings.TrimSpace(id)) {
			return k, true
		}
	}
	return EndpointKind{}, false
}

// Endpoint is an upstream the gateway reaches with an API key. It carries
// no key: KeyHint shows enough of it to recognize it.
type Endpoint struct {
	// ID identifies the endpoint in the cockpit's routes while its kind,
	// name, address and key stay the same.
	ID       string `json:"id"`
	Kind     string `json:"kind"`
	KindName string `json:"kind_name"`
	Name     string `json:"name,omitzero"`
	// BaseURL is the address configured, empty for the kind's usual one;
	// Host is where requests go, for people.
	BaseURL string `json:"base_url,omitzero"`
	Host    string `json:"host"`
	KeyHint string `json:"key_hint,omitzero"`
	Keys    int    `json:"keys"`
	// Models are those the endpoint serves; none serves the kind's known
	// models.
	Models   []EndpointModel `json:"models,omitzero"`
	Prefix   string          `json:"prefix,omitzero"`
	Disabled bool            `json:"disabled,omitzero"`
	State    string          `json:"state"`
	Uptime   Uptime          `json:"uptime"`
	Errors   []ErrorSample   `json:"errors,omitzero"` // newest first

	indexes  []string // the gateway's auth indexes of its keys
	position int      // in its kind's list
	key      string
}

// EndpointModel is a model an endpoint serves, under Alias when it has one.
type EndpointModel struct {
	Name  string `json:"name"`
	Alias string `json:"alias,omitzero"`
}

// EndpointInput is an endpoint as a form describes it.
type EndpointInput struct {
	Kind    string `json:"kind"`
	Name    string `json:"name"`
	BaseURL string `json:"base_url"`
	// APIKey nil keeps the key an endpoint has.
	APIKey *string         `json:"api_key,omitzero"`
	Models []EndpointModel `json:"models"`
	Prefix string          `json:"prefix"`
}

var (
	// ErrNoEndpoint is the answer for an endpoint the gateway does not have.
	ErrNoEndpoint = errors.New("no such endpoint")
	endpointName  = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9._-]{0,39}$`)
)

// endpointID names an endpoint by what makes it the endpoint it is.
func endpointID(kind, name, base, key string) string {
	sum := sha256.Sum256([]byte(kind + "\x00" + name + "\x00" + strings.TrimRight(base, "/") + "\x00" + key))
	return kind + "-" + hex.EncodeToString(sum[:])[:10]
}

// rawList reads one of the gateway's lists as it stores it, every field
// included, so that writing it back loses nothing the cockpit does not know.
func (c *Client) rawList(ctx context.Context, section string) ([]map[string]any, error) {
	var answer map[string][]map[string]any
	if err := c.do(ctx, http.MethodGet, "/v0/management/"+section, nil, &answer); err != nil {
		return nil, err
	}
	list := answer[section]
	for _, entry := range list {
		// The index is the gateway's report, not configuration.
		delete(entry, "auth-index")
		for _, key := range asList(entry["api-key-entries"]) {
			delete(asMap(key), "auth-index")
		}
	}
	return list, nil
}

// putList replaces one of the gateway's lists, and waits until the gateway
// reads it back. The gateway reloads its configuration after every change;
// a list read while a reload of an earlier change is under way can come back
// stale, and a change built on it would undo that earlier one.
func (c *Client) putList(ctx context.Context, kind EndpointKind, list []map[string]any) error {
	if list == nil {
		list = []map[string]any{}
	}
	if err := c.do(ctx, http.MethodPut, "/v0/management/"+kind.section, list, nil); err != nil {
		return err
	}
	want := listSignature(kind, list)
	deadline := time.Now().Add(5 * time.Second)
	for {
		select {
		case <-ctx.Done():
			return ctx.Err()
		case <-time.After(150 * time.Millisecond):
		}
		got, err := c.rawList(ctx, kind.section)
		if err == nil && listSignature(kind, got) == want {
			return nil
		}
		if time.Now().After(deadline) {
			return errors.New("the gateway did not take the change; look at its list again")
		}
	}
}

// listSignature sums up what the cockpit edits in a list.
func listSignature(kind EndpointKind, list []map[string]any) string {
	var b strings.Builder
	for i, entry := range list {
		e := endpointOf(kind, record(entry), i)
		fmt.Fprintf(&b, "%s|%t|%d|", e.ID, e.Disabled, len(e.Models))
		for _, m := range e.Models {
			b.WriteString(m.Name + "=" + m.Alias + ",")
		}
		b.WriteByte('\n')
	}
	return b.String()
}

// endpoints lists every endpoint of every kind, with the gateway's auth
// indexes of their keys.
func (c *Client) endpoints(ctx context.Context) ([]Endpoint, error) {
	var out []Endpoint
	for _, kind := range endpointKinds {
		var answer map[string][]map[string]any
		if err := c.do(ctx, http.MethodGet, "/v0/management/"+kind.section, nil, &answer); err != nil {
			return nil, err
		}
		for i, entry := range answer[kind.section] {
			out = append(out, endpointOf(kind, record(entry), i))
		}
	}
	return out, nil
}

// endpointOf reads one entry of a kind's list.
func endpointOf(kind EndpointKind, r record, position int) Endpoint {
	e := Endpoint{Kind: kind.ID, KindName: kind.Name, BaseURL: strings.TrimRight(r.str("base-url"), "/"), Prefix: r.str("prefix"), position: position}
	if kind.Named {
		e.Name = r.str("name")
		e.Disabled = r.flag("disabled")
		for _, raw := range r.list("api-key-entries") {
			entry := record(asMap(raw))
			if key := entry.str("api-key"); key != "" {
				e.Keys++
				if e.key == "" {
					e.key = key
				}
			}
			if index := entry.str("auth-index"); index != "" {
				e.indexes = append(e.indexes, index)
			}
		}
	} else {
		e.key = r.str("api-key")
		if e.key != "" {
			e.Keys = 1
		}
		if index := r.str("auth-index"); index != "" {
			e.indexes = append(e.indexes, index)
		}
		e.Disabled = slices.ContainsFunc(r.list("excluded-models"), func(v any) bool { s, _ := v.(string); return strings.TrimSpace(s) == "*" })
	}
	for _, raw := range r.list("models") {
		m := record(asMap(raw))
		if name := m.str("name"); name != "" {
			model := EndpointModel{Name: name, Alias: m.str("alias")}
			if model.Alias == name {
				model.Alias = ""
			}
			e.Models = append(e.Models, model)
		}
	}
	e.KeyHint = keyHint(e.key)
	base := e.BaseURL
	if base == "" {
		base = kind.BaseURL
	}
	e.Host = base
	if u, err := url.Parse(base); err == nil && u.Host != "" {
		e.Host = u.Host
	}
	e.ID = endpointID(kind.ID, e.Name, e.BaseURL, e.key)
	e.State = StateReady
	if e.Disabled {
		e.State = StateDisabled
	}
	return e
}

func keyHint(key string) string {
	switch {
	case key == "":
		return ""
	case len(key) < 12:
		return "set"
	default:
		return "…" + key[len(key)-4:]
	}
}

// entryOf writes an endpoint as its kind's list stores it, over the fields
// of the entry it replaces, if any.
func entryOf(kind EndpointKind, in EndpointInput, key string, old map[string]any) map[string]any {
	entry := map[string]any{}
	for k, v := range old {
		entry[k] = v
	}
	base := strings.TrimRight(strings.TrimSpace(in.BaseURL), "/")
	if base == "" && kind.fillBase {
		base = kind.BaseURL
	}
	models := make([]any, 0, len(in.Models))
	for _, m := range in.Models {
		alias := strings.TrimSpace(m.Alias)
		if alias == "" {
			alias = strings.TrimSpace(m.Name)
		}
		models = append(models, map[string]any{"name": strings.TrimSpace(m.Name), "alias": alias})
	}
	if kind.Named {
		entry["name"] = strings.TrimSpace(in.Name)
		entry["base-url"] = base
		keys := []any{}
		// Further keys of the entry stay; the first is the one the form edits.
		for i, raw := range asList(old["api-key-entries"]) {
			if i > 0 {
				keys = append(keys, raw)
			}
		}
		entry["api-key-entries"] = append([]any{map[string]any{"api-key": key}}, keys...)
	} else {
		entry["api-key"] = key
		if base == "" {
			delete(entry, "base-url")
		} else {
			entry["base-url"] = base
		}
	}
	if len(models) > 0 || kind.NeedsModels {
		entry["models"] = models
	} else {
		delete(entry, "models")
	}
	if prefix := strings.TrimSpace(in.Prefix); prefix != "" {
		entry["prefix"] = prefix
	} else {
		delete(entry, "prefix")
	}
	return entry
}

// validate checks an endpoint before the gateway gets it.
func (in EndpointInput) validate(kind EndpointKind, key string) error {
	base := strings.TrimSpace(in.BaseURL)
	switch {
	case kind.Named && !endpointName.MatchString(strings.TrimSpace(in.Name)):
		return errors.New("name the endpoint with letters, digits, dots, dashes or underscores, such as openrouter")
	case kind.NeedsBase && base == "":
		return errors.New("enter the endpoint's base URL, such as https://openrouter.ai/api/v1")
	case strings.TrimSpace(key) == "" && kind.ID != "openai-compatible":
		return errors.New("enter the endpoint's API key")
	case kind.NeedsModels && len(in.Models) == 0:
		return errors.New("list the models the endpoint serves: fetch them, or type their names")
	case strings.ContainsFunc(key, func(r rune) bool { return r <= ' ' || r == 0x7f }):
		return errors.New("the API key must be a single token without spaces")
	}
	if base != "" {
		u, err := url.Parse(base)
		if err != nil || (u.Scheme != "http" && u.Scheme != "https") || u.Host == "" || u.User != nil || u.RawQuery != "" {
			return fmt.Errorf("base URL %q must be an http(s) URL", base)
		}
	}
	for _, m := range in.Models {
		if strings.TrimSpace(m.Name) == "" || strings.ContainsAny(m.Name+m.Alias, " \t\r\n") {
			return fmt.Errorf("model %q must be an ID without spaces", m.Name)
		}
	}
	return nil
}

// findEndpoint finds an endpoint by ID, with the raw list of its kind.
func (h *Host) findEndpoint(ctx context.Context, id string) (EndpointKind, Endpoint, []map[string]any, *Client, error) {
	client, err := h.running()
	if err != nil {
		return EndpointKind{}, Endpoint{}, nil, nil, err
	}
	kindID, _, _ := strings.Cut(id, "-")
	for _, kind := range endpointKinds {
		if !strings.HasPrefix(id, kind.ID+"-") && kind.ID != kindID {
			continue
		}
		list, err := client.rawList(ctx, kind.section)
		if err != nil {
			return kind, Endpoint{}, nil, nil, err
		}
		for i, entry := range list {
			if e := endpointOf(kind, record(entry), i); e.ID == id {
				return kind, e, list, client, nil
			}
		}
	}
	return EndpointKind{}, Endpoint{}, nil, nil, ErrNoEndpoint
}

// AddEndpoint adds an upstream to the gateway.
func (h *Host) AddEndpoint(ctx context.Context, in EndpointInput) (Endpoint, error) {
	h.edits.Lock()
	defer h.edits.Unlock()
	kind, ok := endpointKind(in.Kind)
	if !ok {
		return Endpoint{}, fmt.Errorf("unknown kind of endpoint %q", in.Kind)
	}
	client, err := h.running()
	if err != nil {
		return Endpoint{}, err
	}
	key := ""
	if in.APIKey != nil {
		key = strings.TrimSpace(*in.APIKey)
	}
	if err := in.validate(kind, key); err != nil {
		return Endpoint{}, err
	}
	list, err := client.rawList(ctx, kind.section)
	if err != nil {
		return Endpoint{}, err
	}
	entry := entryOf(kind, in, key, nil)
	added := endpointOf(kind, record(entry), len(list))
	for i, existing := range list {
		other := endpointOf(kind, record(existing), i)
		switch {
		case kind.Named && strings.EqualFold(other.Name, added.Name):
			return Endpoint{}, fmt.Errorf("an endpoint is named %s already", other.Name)
		case other.ID == added.ID:
			return Endpoint{}, errors.New("the gateway has this endpoint already")
		}
	}
	if err := client.putList(ctx, kind, append(list, entry)); err != nil {
		return Endpoint{}, err
	}
	return added, nil
}

// UpdateEndpoint changes an endpoint; a nil key keeps the one it has.
func (h *Host) UpdateEndpoint(ctx context.Context, id string, in EndpointInput) (Endpoint, error) {
	h.edits.Lock()
	defer h.edits.Unlock()
	kind, current, list, client, err := h.findEndpoint(ctx, id)
	if err != nil {
		return Endpoint{}, err
	}
	in.Kind = kind.ID
	key := current.key
	if in.APIKey != nil {
		key = strings.TrimSpace(*in.APIKey)
	}
	if err := in.validate(kind, key); err != nil {
		return Endpoint{}, err
	}
	entry := entryOf(kind, in, key, list[current.position])
	if kind.Named {
		for i, existing := range list {
			if i != current.position && strings.EqualFold(record(existing).str("name"), strings.TrimSpace(in.Name)) {
				return Endpoint{}, fmt.Errorf("an endpoint is named %s already", in.Name)
			}
		}
	}
	list[current.position] = entry
	if err := client.putList(ctx, kind, list); err != nil {
		return Endpoint{}, err
	}
	return endpointOf(kind, record(entry), current.position), nil
}

// RemoveEndpoint takes an endpoint out of the gateway, with its history.
func (h *Host) RemoveEndpoint(ctx context.Context, id string) error {
	h.edits.Lock()
	defer h.edits.Unlock()
	kind, current, list, client, err := h.findEndpoint(ctx, id)
	if err != nil {
		return err
	}
	if err := client.putList(ctx, kind, slices.Delete(list, current.position, current.position+1)); err != nil {
		return err
	}
	for _, index := range current.indexes {
		h.history.ForgetIndex(index)
	}
	return nil
}

// SetEndpointDisabled turns an endpoint off or back on. A named endpoint
// has a switch of its own; a key is off while it excludes every model.
func (h *Host) SetEndpointDisabled(ctx context.Context, id string, disabled bool) error {
	h.edits.Lock()
	defer h.edits.Unlock()
	kind, current, list, client, err := h.findEndpoint(ctx, id)
	if err != nil {
		return err
	}
	entry := list[current.position]
	if kind.Named {
		entry["disabled"] = disabled
	} else {
		excluded := []any{}
		for _, v := range asList(entry["excluded-models"]) {
			if s, _ := v.(string); strings.TrimSpace(s) != "*" {
				excluded = append(excluded, v)
			}
		}
		if disabled {
			excluded = append(excluded, "*")
		}
		entry["excluded-models"] = excluded
	}
	return client.putList(ctx, kind, list)
}

// ProbeEndpoint asks an upstream which models it serves, with the key the
// form gives or, for an endpoint the gateway has, with its own.
func (h *Host) ProbeEndpoint(ctx context.Context, id string, in EndpointInput) ([]string, error) {
	kind, ok := endpointKind(in.Kind)
	key := ""
	if in.APIKey != nil {
		key = strings.TrimSpace(*in.APIKey)
	}
	if id != "" {
		found, current, _, _, err := h.findEndpoint(ctx, id)
		if err != nil {
			return nil, err
		}
		kind, ok = found, true
		if in.APIKey == nil {
			key = current.key
		}
	}
	if !ok {
		return nil, fmt.Errorf("unknown kind of endpoint %q", in.Kind)
	}
	base := strings.TrimRight(strings.TrimSpace(in.BaseURL), "/")
	if base == "" {
		base = kind.BaseURL
	}
	if base == "" {
		return nil, errors.New("enter the endpoint's base URL")
	}
	ctx, cancel := context.WithTimeout(ctx, 20*time.Second)
	defer cancel()
	return probeModels(ctx, kind.probe, base, key)
}

// probeModels lists what an upstream serves, as its API lists models.
func probeModels(ctx context.Context, style, base, key string) ([]string, error) {
	var models []string
	switch style {
	case "anthropic":
		attempts := 1
		list, err := anthropic.ListModels(ctx, anthropic.Config{APIKey: key, BaseURL: base, MaxAttempts: &attempts})
		if err != nil {
			return nil, err
		}
		models = list
	case "gemini":
		var answer struct {
			Models []struct {
				Name string `json:"name"`
			} `json:"models"`
		}
		endpoint := base + "/v1beta/models?pageSize=1000&key=" + url.QueryEscape(key)
		if err := getJSON(ctx, endpoint, nil, &answer); err != nil {
			return nil, err
		}
		for _, m := range answer.Models {
			models = append(models, strings.TrimPrefix(m.Name, "models/"))
		}
	default:
		var answer struct {
			Data []struct {
				ID string `json:"id"`
			} `json:"data"`
		}
		header := http.Header{}
		if key != "" {
			header.Set("Authorization", "Bearer "+key)
		}
		if err := getJSON(ctx, base+"/models", header, &answer); err != nil {
			return nil, err
		}
		for _, m := range answer.Data {
			models = append(models, m.ID)
		}
	}
	models = slices.DeleteFunc(models, func(m string) bool { return strings.TrimSpace(m) == "" })
	slices.Sort(models)
	return slices.Compact(models), nil
}

func getJSON(ctx context.Context, endpoint string, header http.Header, out any) error {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, endpoint, nil)
	if err != nil {
		return fmt.Errorf("invalid base URL: %w", err)
	}
	for k, v := range header {
		req.Header[k] = v
	}
	req.Header.Set("Accept", "application/json")
	res, err := http.DefaultClient.Do(req)
	if err != nil {
		return fmt.Errorf("cannot reach the endpoint: %w", redactKey(err))
	}
	defer res.Body.Close()
	body, err := io.ReadAll(io.LimitReader(res.Body, 8<<20))
	if err != nil {
		return err
	}
	switch {
	case res.StatusCode == http.StatusUnauthorized || res.StatusCode == http.StatusForbidden:
		return fmt.Errorf("the endpoint rejected the API key (%s)", res.Status)
	case res.StatusCode == http.StatusNotFound:
		return errors.New("the endpoint has no model list there; check the base URL")
	case res.StatusCode != http.StatusOK:
		if message := errorMessage(string(body)); message != "" {
			return fmt.Errorf("the endpoint answered %s: %s", res.Status, message)
		}
		return fmt.Errorf("the endpoint answered %s", res.Status)
	}
	if err := json.Unmarshal(body, out); err != nil {
		return errors.New("the endpoint's model list is not in the format its kind uses; check the kind and the base URL")
	}
	return nil
}

// redactKey keeps a key given in a query string out of an error.
func redactKey(err error) error {
	var urlErr *url.Error
	if errors.As(err, &urlErr) {
		if u, parseErr := url.Parse(urlErr.URL); parseErr == nil && u.Query().Has("key") {
			q := u.Query()
			q.Set("key", "…")
			u.RawQuery = q.Encode()
			return &url.Error{Op: urlErr.Op, URL: u.String(), Err: urlErr.Err}
		}
	}
	return err
}

func asList(v any) []any {
	list, _ := v.([]any)
	return list
}
