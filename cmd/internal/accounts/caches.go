package accounts

import (
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"slices"
	"strings"
	"sync"
)

// Prompt-cache TTLs. A model can explicitly ask for a five-minute or one-hour
// cache instead of leaving the TTL to its provider. In particular, an explicit
// five-minute breakpoint survives the gateway's Claude OAuth path, which would
// otherwise raise a breakpoint without a TTL to one hour.
//
// The gateway owns the book: each setting takes effect on the model's next run.
// No setting is the provider's default, not necessarily five minutes.
const (
	CacheTTL5m = "5m"
	CacheTTL1h = "1h"

	maxCacheTTLs = 1000
)

// CacheTTL is the prompt-cache TTL the user set for a model.
type CacheTTL struct {
	// Model is the gateway's ID of the model, in lower case.
	Model string `json:"model"`
	TTL   string `json:"ttl"`
}

// CacheBook keeps the cache TTLs the user set, in cache-ttls.json beside the
// gateway's config. A run takes its model's TTL through
// runconfig.CacheTTLEnvironment.
type CacheBook struct {
	path string

	mu   sync.Mutex
	ttls []CacheTTL
	err  error // why the file could not be read
}

type cacheFile struct {
	Version int        `json:"version"`
	TTLs    []CacheTTL `json:"ttls"`
}

// NewCacheBook reads the TTLs kept at path.
func NewCacheBook(path string) *CacheBook {
	b := &CacheBook{path: path}
	if path == "" {
		return b
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return b
	}
	var saved cacheFile
	if err == nil {
		err = json.Unmarshal(data, &saved)
	}
	if err == nil && saved.Version != 1 {
		err = fmt.Errorf("version %d", saved.Version)
	}
	if err != nil {
		b.err = fmt.Errorf("%s: %w", path, err)
		return b
	}
	for _, ttl := range saved.TTLs {
		ttl, err := cleanCacheTTL(ttl)
		if err == nil && !slices.ContainsFunc(b.ttls, func(x CacheTTL) bool { return x.Model == ttl.Model }) {
			b.ttls = append(b.ttls, ttl)
		}
	}
	return b
}

// Resolve is the TTL the user set for a model; empty when none is.
func (b *CacheBook) Resolve(model string) string {
	if b == nil {
		return ""
	}
	key := contextKey(model)
	b.mu.Lock()
	defer b.mu.Unlock()
	for _, ttl := range b.ttls {
		if ttl.Model == key {
			return ttl.TTL
		}
	}
	return ""
}

// TTLs lists the TTLs the user set, by model.
func (b *CacheBook) TTLs() []CacheTTL {
	if b == nil {
		return nil
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	out := slices.Clone(b.ttls)
	slices.SortFunc(out, func(x, y CacheTTL) int { return strings.Compare(x.Model, y.Model) })
	return out
}

// Err says why the TTLs kept could not be read; saving replaces them.
func (b *CacheBook) Err() error {
	if b == nil {
		return nil
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.err
}

// Set sets a model's TTL, or replaces the one it has.
func (b *CacheBook) Set(ttl CacheTTL) (CacheTTL, error) {
	ttl, err := cleanCacheTTL(ttl)
	if err != nil {
		return CacheTTL{}, err
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	ttls := slices.Clone(b.ttls)
	if i := slices.IndexFunc(ttls, func(x CacheTTL) bool { return x.Model == ttl.Model }); i >= 0 {
		ttls[i] = ttl
	} else if len(ttls) >= maxCacheTTLs {
		return CacheTTL{}, fmt.Errorf("at most %d cache TTLs", maxCacheTTLs)
	} else {
		ttls = append(ttls, ttl)
	}
	if err := b.save(ttls); err != nil {
		return CacheTTL{}, err
	}
	b.ttls = ttls
	return ttl, nil
}

// Remove drops a model's TTL: its runs go back to the provider's default.
func (b *CacheBook) Remove(model string) error {
	key := contextKey(model)
	b.mu.Lock()
	defer b.mu.Unlock()
	ttls := slices.DeleteFunc(slices.Clone(b.ttls), func(ttl CacheTTL) bool { return ttl.Model == key })
	if len(ttls) == len(b.ttls) {
		return fmt.Errorf("no cache TTL of yours for %q", key)
	}
	if err := b.save(ttls); err != nil {
		return err
	}
	b.ttls = ttls
	return nil
}

func (b *CacheBook) save(ttls []CacheTTL) error {
	if b.path == "" {
		return nil
	}
	file := cacheFile{Version: 1, TTLs: ttls}
	if file.TTLs == nil {
		file.TTLs = []CacheTTL{}
	}
	data, err := json.Marshal(file, json.Deterministic(true))
	if err != nil {
		return err
	}
	if err := writeFileAtomic(b.path, data); err != nil {
		return err
	}
	b.err = nil
	return nil
}

func cleanCacheTTL(ttl CacheTTL) (CacheTTL, error) {
	ttl.Model = contextKey(ttl.Model)
	ttl.TTL = strings.ToLower(strings.TrimSpace(ttl.TTL))
	switch {
	case ttl.Model == "":
		return ttl, errors.New("a cache TTL names a model")
	case strings.Contains(ttl.Model, "*"):
		return ttl, errors.New("a cache TTL names one model, not a pattern")
	case len(ttl.Model) > 200 || strings.ContainsAny(ttl.Model, " \t\r\n"):
		return ttl, errors.New("a model's ID has no spaces")
	case ttl.TTL != CacheTTL5m && ttl.TTL != CacheTTL1h:
		return ttl, errors.New("a cache TTL is 5m or 1h")
	}
	return ttl, nil
}

// CacheTTL is the cache TTL the user set for a model's runs; empty when none
// is and the provider's default holds.
func (h *Host) CacheTTL(model string) string { return h.caches.Resolve(model) }

// SetCacheTTL sets the prompt-cache TTL of a model's runs.
func (h *Host) SetCacheTTL(ttl CacheTTL) (CacheTTL, error) { return h.caches.Set(ttl) }

// RemoveCacheTTL drops the cache TTL the user set for a model.
func (h *Host) RemoveCacheTTL(model string) error { return h.caches.Remove(model) }
