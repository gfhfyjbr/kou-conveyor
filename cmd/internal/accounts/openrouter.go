package accounts

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"net/http"
	"os"
	"regexp"
	"strconv"
	"strings"
	"sync"
	"time"
)

// OpenRouter lists the models of every provider it resells with what their
// tokens cost, at the providers' own prices, and follows new models within
// days. The cockpit fetches the list twice a day and prices a model by it
// unless the user set a price: before the prices built in, which age.
// KOU_CONVEYOR_OPENROUTER_PRICES=off keeps the cockpit from asking.

const (
	openRouterURL   = "https://openrouter.ai/api/v1/models"
	openRouterEvery = 12 * time.Hour
	openRouterRetry = 30 * time.Minute
)

// OpenRouterStatus is how the list stands, for the price editor.
type OpenRouterStatus struct {
	Enabled bool      `json:"enabled"`
	Fetched time.Time `json:"fetched,omitzero"`
	Models  int       `json:"models"`
	Error   string    `json:"error,omitzero"`
}

type openRouterEntry struct {
	ID string `json:"id"` // OpenRouter's, such as anthropic/claude-opus-4.5
	Price
}

// OpenRouter keeps OpenRouter's prices, in a file between fetches.
type OpenRouter struct {
	path    string
	url     string
	client  *http.Client
	enabled bool

	mu      sync.Mutex
	byKey   map[string]openRouterEntry
	fetched time.Time
	err     string
}

type openRouterFile struct {
	Version int               `json:"version"`
	Fetched time.Time         `json:"fetched"`
	Models  []openRouterEntry `json:"models"`
}

// NewOpenRouter reads the prices kept at path.
func NewOpenRouter(path string) *OpenRouter {
	o := &OpenRouter{path: path, url: openRouterURL, client: &http.Client{Timeout: 30 * time.Second},
		enabled: !strings.EqualFold(strings.TrimSpace(os.Getenv("KOU_CONVEYOR_OPENROUTER_PRICES")), "off"), byKey: map[string]openRouterEntry{}}
	var saved openRouterFile
	if data, err := os.ReadFile(path); err == nil && json.Unmarshal(data, &saved) == nil && saved.Version == 1 {
		o.index(saved.Models)
		o.fetched = saved.Fetched
	}
	return o
}

func (o *OpenRouter) index(models []openRouterEntry) {
	o.byKey = map[string]openRouterEntry{}
	for _, m := range models {
		o.byKey[openRouterKey(m.ID)] = m
	}
}

var dateSuffix = regexp.MustCompile(`-(20\d{6}|20\d{2}-\d{2}-\d{2})$`)

// openRouterKey is a model's ID as the list is looked up by: without the
// provider's path or a date, dots as dashes.
func openRouterKey(id string) string {
	return dateSuffix.ReplaceAllString(builtinKey(id), "")
}

// Lookup is OpenRouter's price of a model, if it lists it.
func (o *OpenRouter) Lookup(model string) (ModelPrice, bool) {
	if o == nil {
		return ModelPrice{}, false
	}
	o.mu.Lock()
	defer o.mu.Unlock()
	e, ok := o.byKey[openRouterKey(model)]
	if !ok {
		return ModelPrice{}, false
	}
	return ModelPrice{Price: e.Price, Source: PriceOpenRouter, Rule: e.ID}, true
}

// Status says how the list stands.
func (o *OpenRouter) Status() OpenRouterStatus {
	o.mu.Lock()
	defer o.mu.Unlock()
	return OpenRouterStatus{Enabled: o.enabled, Fetched: o.fetched, Models: len(o.byKey), Error: o.err}
}

// Refresh fetches the list now.
func (o *OpenRouter) Refresh(ctx context.Context) error {
	if !o.enabled {
		return errors.New("fetching OpenRouter's prices is off (KOU_CONVEYOR_OPENROUTER_PRICES=off)")
	}
	models, err := o.fetch(ctx)
	o.mu.Lock()
	if err != nil {
		o.err = err.Error()
		o.mu.Unlock()
		return err
	}
	o.index(models)
	o.fetched, o.err = time.Now().UTC(), ""
	file := openRouterFile{Version: 1, Fetched: o.fetched, Models: models}
	o.mu.Unlock()
	if o.path == "" {
		return nil
	}
	data, err := json.Marshal(file)
	if err != nil {
		return err
	}
	return writeFileAtomic(o.path, data)
}

func (o *OpenRouter) fetch(ctx context.Context) ([]openRouterEntry, error) {
	req, err := http.NewRequestWithContext(ctx, http.MethodGet, o.url, nil)
	if err != nil {
		return nil, err
	}
	req.Header.Set("Accept", "application/json")
	res, err := o.client.Do(req)
	if err != nil {
		return nil, fmt.Errorf("OpenRouter: %w", err)
	}
	defer res.Body.Close()
	if res.StatusCode != http.StatusOK {
		return nil, fmt.Errorf("OpenRouter answered %s", res.Status)
	}
	var body struct {
		Data []struct {
			ID      string         `json:"id"`
			Pricing map[string]any `json:"pricing"`
		} `json:"data"`
	}
	if err := json.UnmarshalRead(io.LimitReader(res.Body, 32<<20), &body); err != nil {
		return nil, fmt.Errorf("OpenRouter: %w", err)
	}
	var out []openRouterEntry
	for _, m := range body.Data {
		// Variants (":free", ":batch", ":thinking") and aliases ("~…") are
		// not the list price of the model.
		if m.ID == "" || strings.ContainsAny(m.ID, ":~") {
			continue
		}
		perToken := func(key string) (float64, bool) {
			var v float64
			var err error
			switch x := m.Pricing[key].(type) {
			case string:
				v, err = strconv.ParseFloat(x, 64)
			case float64:
				v = x
			default:
				return 0, false
			}
			if err != nil || v < 0 {
				return 0, false
			}
			return round(v * 1e6), true
		}
		input, ok := perToken("prompt")
		output, ok2 := perToken("completion")
		if !ok || !ok2 || input == 0 && output == 0 {
			continue // free or unpriced
		}
		p := Price{Input: input, Output: output, CacheRead: input, CacheWrite: input}
		if v, ok := perToken("input_cache_read"); ok {
			p.CacheRead = v
		}
		// The gateway's Claude requests cache for an hour.
		if v, ok := perToken("input_cache_write_1h"); ok {
			p.CacheWrite = v
		} else if v, ok := perToken("input_cache_write"); ok {
			p.CacheWrite = v
		}
		out = append(out, openRouterEntry{ID: m.ID, Price: p})
	}
	if len(out) == 0 {
		return nil, errors.New("OpenRouter listed no priced models")
	}
	return out, nil
}

// keep fetches the list when it is older than half a day, until ctx ends.
func (o *OpenRouter) keep(ctx context.Context) {
	if !o.enabled {
		return
	}
	for {
		o.mu.Lock()
		due := time.Until(o.fetched.Add(openRouterEvery))
		o.mu.Unlock()
		if due <= 0 {
			if err := o.Refresh(ctx); err != nil {
				due = openRouterRetry
			} else {
				due = openRouterEvery
			}
		}
		select {
		case <-ctx.Done():
			return
		case <-time.After(due):
		}
	}
}
