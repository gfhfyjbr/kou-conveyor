package accounts

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"slices"
	"strings"
	"sync"
	"time"
)

// Context windows. A run compacts its conversation before it outgrows the
// model's context window: the window the runner knows for the models of the
// providers it knows, else one it assumes. The user sets a model's window on
// the Accounts page, typed or as OpenRouter lists it, and the model's runs
// take it (runconfig.ContextWindowEnvironment) over the runner's. The
// windows are kept beside the gateway's config, in context-windows.json.

const (
	// MinContextWindow and MaxContextWindow bound the windows the user sets:
	// fewer than a thousand tokens is a slip, and no model holds a hundred
	// million.
	MinContextWindow = 1_000
	MaxContextWindow = 100_000_000
	// maxContextWindows bounds the windows the user keeps.
	maxContextWindows = 1000
	// openRouterFresh is how old OpenRouter's list may be when a window is
	// taken from it; an older one is fetched again first.
	openRouterFresh = 10 * time.Minute
)

var (
	// ErrOpenRouterOff: the cockpit does not ask OpenRouter.
	ErrOpenRouterOff = errors.New("fetching from OpenRouter is off (KOU_CONVEYOR_OPENROUTER_PRICES=off)")
	// ErrNotOnOpenRouter: OpenRouter lists no context window for a model.
	ErrNotOnOpenRouter = errors.New("OpenRouter lists no context window for")
)

// ContextWindow is the context window the user set for a model.
type ContextWindow struct {
	// Model is the gateway's ID of the model, in lower case.
	Model  string `json:"model"`
	Tokens int64  `json:"tokens"`
	// OpenRouter is the ID OpenRouter lists the model under, when the window
	// was taken from its list.
	OpenRouter string `json:"openrouter,omitzero"`
}

// ContextBook keeps the context windows the user set, in a file.
type ContextBook struct {
	path string

	mu      sync.Mutex
	windows []ContextWindow
	err     error // why the file could not be read
}

type contextFile struct {
	Version int             `json:"version"`
	Windows []ContextWindow `json:"windows"`
}

// NewContextBook reads the windows kept at path.
func NewContextBook(path string) *ContextBook {
	b := &ContextBook{path: path}
	if path == "" {
		return b
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return b
	}
	var saved contextFile
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
	for _, w := range saved.Windows {
		w, err := cleanContextWindow(w)
		if err == nil && !slices.ContainsFunc(b.windows, func(x ContextWindow) bool { return x.Model == w.Model }) {
			b.windows = append(b.windows, w)
		}
	}
	return b
}

// contextKey is a model's ID as the windows are kept by.
func contextKey(model string) string { return strings.ToLower(strings.TrimSpace(model)) }

// Resolve is the window the user set for a model; 0 when none is.
func (b *ContextBook) Resolve(model string) int64 {
	if b == nil {
		return 0
	}
	key := contextKey(model)
	b.mu.Lock()
	defer b.mu.Unlock()
	for _, w := range b.windows {
		if w.Model == key {
			return w.Tokens
		}
	}
	return 0
}

// Windows lists the windows the user set, by model.
func (b *ContextBook) Windows() []ContextWindow {
	b.mu.Lock()
	defer b.mu.Unlock()
	out := slices.Clone(b.windows)
	slices.SortFunc(out, func(x, y ContextWindow) int { return strings.Compare(x.Model, y.Model) })
	return out
}

// Err says why the windows kept could not be read; saving replaces them.
func (b *ContextBook) Err() error {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.err
}

// Set sets a model's window, or replaces the one it has.
func (b *ContextBook) Set(w ContextWindow) (ContextWindow, error) {
	w, err := cleanContextWindow(w)
	if err != nil {
		return ContextWindow{}, err
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	windows := slices.Clone(b.windows)
	if i := slices.IndexFunc(windows, func(x ContextWindow) bool { return x.Model == w.Model }); i >= 0 {
		windows[i] = w
	} else if len(windows) >= maxContextWindows {
		return ContextWindow{}, fmt.Errorf("at most %d context windows", maxContextWindows)
	} else {
		windows = append(windows, w)
	}
	if err := b.save(windows); err != nil {
		return ContextWindow{}, err
	}
	b.windows = windows
	return w, nil
}

// Remove drops a model's window: its runs go back to the window the runner
// knows or assumes.
func (b *ContextBook) Remove(model string) error {
	key := contextKey(model)
	b.mu.Lock()
	defer b.mu.Unlock()
	windows := slices.DeleteFunc(slices.Clone(b.windows), func(w ContextWindow) bool { return w.Model == key })
	if len(windows) == len(b.windows) {
		return fmt.Errorf("no context window of yours for %q", key)
	}
	if err := b.save(windows); err != nil {
		return err
	}
	b.windows = windows
	return nil
}

func (b *ContextBook) save(windows []ContextWindow) error {
	if b.path == "" {
		return nil
	}
	file := contextFile{Version: 1, Windows: windows}
	if file.Windows == nil {
		file.Windows = []ContextWindow{}
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

// cleanContextWindow checks a window: one model's ID, and a size a model
// could hold.
func cleanContextWindow(w ContextWindow) (ContextWindow, error) {
	w.Model = contextKey(w.Model)
	w.OpenRouter = strings.TrimSpace(w.OpenRouter)
	switch {
	case w.Model == "":
		return w, errors.New("a context window names a model")
	case strings.Contains(w.Model, "*"):
		return w, errors.New("a context window names one model, not a pattern")
	case len(w.Model) > 200 || strings.ContainsAny(w.Model, " \t\r\n"):
		return w, errors.New("a model's ID has no spaces")
	case len(w.OpenRouter) > 200 || strings.ContainsAny(w.OpenRouter, " \t\r\n"):
		return w, errors.New("OpenRouter's ID of a model has no spaces")
	case w.Tokens < MinContextWindow || w.Tokens > MaxContextWindow:
		return w, fmt.Errorf("a context window holds from %d to %d tokens", MinContextWindow, MaxContextWindow)
	}
	return w, nil
}

// ContextWindow is the context window the user set for a model, which its
// runs take; 0 when none is.
func (h *Host) ContextWindow(model string) int64 { return h.contexts.Resolve(model) }

// SetContextWindow sets the context window of a model's runs.
func (h *Host) SetContextWindow(w ContextWindow) (ContextWindow, error) { return h.contexts.Set(w) }

// RemoveContextWindow drops the context window the user set for a model.
func (h *Host) RemoveContextWindow(model string) error { return h.contexts.Remove(model) }

// ContextModel is a model the editor of context windows lists: one the
// gateway serves or the user set a window for.
type ContextModel struct {
	ID   string `json:"id"`
	Name string `json:"name,omitzero"`
	// Served: the gateway serves it now.
	Served bool `json:"served,omitzero"`
	// Window is the window the user set, which the model's runs take; 0 when
	// none is, and the runner's own holds. From is the ID OpenRouter lists
	// the model under, when the window was taken from its list.
	Window int64  `json:"window,omitzero"`
	From   string `json:"from,omitzero"`
	// Gateway is the window the gateway's model registry gives the model.
	Gateway int64 `json:"gateway,omitzero"`
	// OpenRouter is the window OpenRouter's list, as kept, gives the model,
	// which it lists under OpenRouterID.
	OpenRouter   int64  `json:"openrouter,omitzero"`
	OpenRouterID string `json:"openrouter_id,omitzero"`
}

// ContextListing is what the editor of context windows shows.
type ContextListing struct {
	// Models are those the gateway serves, then those only a window is
	// kept for.
	Models     []ContextModel   `json:"models"`
	OpenRouter OpenRouterStatus `json:"openrouter"`
	// Error says why the gateway's models, or the windows kept, could not
	// be read.
	Error string `json:"error,omitzero"`
}

// ContextWindows lists the models the gateway serves and those the user set
// a window for, with the windows the user, the gateway and OpenRouter give
// them.
func (h *Host) ContextWindows(ctx context.Context) ContextListing {
	listing := ContextListing{Models: []ContextModel{}, OpenRouter: h.market.Status()}
	var problems []string
	if err := h.contexts.Err(); err != nil {
		problems = append(problems, "the context windows kept could not be read: "+err.Error())
	}
	served, err := h.Models(ctx)
	if err != nil {
		problems = append(problems, err.Error())
	}
	at := map[string]int{}
	entry := func(id string) *ContextModel {
		key := contextKey(id)
		i, ok := at[key]
		if !ok {
			i = len(listing.Models)
			at[key] = i
			listing.Models = append(listing.Models, ContextModel{ID: id})
		}
		return &listing.Models[i]
	}
	for _, m := range served {
		c := entry(m.ID)
		c.Name, c.Served, c.Gateway = m.Name, true, m.Context
	}
	for _, w := range h.contexts.Windows() {
		c := entry(w.Model)
		c.Window, c.From = w.Tokens, w.OpenRouter
	}
	for i := range listing.Models {
		c := &listing.Models[i]
		c.OpenRouter, c.OpenRouterID, _ = h.market.Window(c.ID)
	}
	slices.SortStableFunc(listing.Models, func(x, y ContextModel) int {
		if x.Served != y.Served {
			if x.Served {
				return -1
			}
			return 1
		}
		return strings.Compare(x.ID, y.ID)
	})
	listing.Error = strings.Join(problems, "; ")
	return listing
}

// OpenRouterWindow is the context window OpenRouter lists for a model.
type OpenRouterWindow struct {
	Model  string `json:"model"`
	Tokens int64  `json:"tokens"`
	// OpenRouter is the ID OpenRouter lists the model under.
	OpenRouter string    `json:"openrouter"`
	Fetched    time.Time `json:"fetched"`
	// Note says OpenRouter could not be reached, and the window is of the
	// list fetched before.
	Note string `json:"note,omitzero"`
}

// OpenRouterWindow gets the context window OpenRouter lists for a model,
// fetching OpenRouter's list first unless that was minutes ago. When
// OpenRouter cannot be reached, the list kept answers if it can, and says
// so.
func (h *Host) OpenRouterWindow(ctx context.Context, model string) (OpenRouterWindow, error) {
	model = strings.TrimSpace(model)
	if !h.market.enabled {
		return OpenRouterWindow{}, ErrOpenRouterOff
	}
	var note string
	if !h.market.fresh(openRouterFresh) {
		if err := h.market.Refresh(ctx); err != nil {
			if _, _, ok := h.market.Window(model); !ok {
				return OpenRouterWindow{}, err
			}
			note = "OpenRouter could not be reached (" + err.Error() + "): the window is from its list fetched before"
		}
	}
	tokens, id, ok := h.market.Window(model)
	if !ok {
		return OpenRouterWindow{}, fmt.Errorf("%w %s", ErrNotOnOpenRouter, model)
	}
	return OpenRouterWindow{Model: model, Tokens: tokens, OpenRouter: id, Fetched: h.market.Status().Fetched, Note: note}, nil
}
