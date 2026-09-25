package main

import (
	"encoding/json"
	"errors"
	"fmt"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

const historyLimit = 200

type historyEntry struct {
	Prompt, SessionID, Title, Model string
	CreatedAt                       time.Time
}

func loadHistory(path string) ([]historyEntry, error) {
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return nil, nil
	}
	if err != nil {
		return nil, fmt.Errorf("read history: %w", err)
	}
	var result []historyEntry
	if len(data) > 0 {
		if err := json.Unmarshal(data, &result); err != nil {
			return nil, fmt.Errorf("decode history: %w", err)
		}
	}
	// The file is shared by every cockpit in the workspace, and a checkout
	// can ship one: nothing in it may reach the terminal as an escape.
	for i := range result {
		h := &result[i]
		h.Prompt, h.Title, h.Model, h.SessionID = cockpit.Clean(h.Prompt), cockpit.Clean(h.Title), cockpit.Clean(h.Model), cockpit.Clean(h.SessionID)
	}
	return result, nil
}

// saveHistory merges history with whatever other terminals saved since it
// was loaded, writes the result atomically and returns it. Two cockpits in
// one workspace therefore never erase each other's prompts.
func saveHistory(path string, history []historyEntry) ([]historyEntry, error) {
	onDisk, err := loadHistory(path)
	if err != nil {
		onDisk = nil // a corrupt file is replaced rather than blocking every save
	}
	type key struct {
		at     int64
		prompt string
	}
	seen := make(map[key]bool, len(onDisk)+len(history))
	var merged []historyEntry
	for _, h := range append(onDisk, history...) {
		k := key{h.CreatedAt.UnixNano(), h.Prompt}
		if !seen[k] {
			seen[k] = true
			merged = append(merged, h)
		}
	}
	slices.SortStableFunc(merged, func(a, b historyEntry) int { return a.CreatedAt.Compare(b.CreatedAt) })
	if len(merged) > historyLimit {
		merged = merged[len(merged)-historyLimit:]
	}
	data, err := json.MarshalIndent(merged, "", "  ")
	if err != nil {
		return history, err
	}
	if err := os.MkdirAll(filepath.Dir(path), 0o700); err != nil {
		return history, err
	}
	tmp, err := os.CreateTemp(filepath.Dir(path), ".ui-history-*.tmp")
	if err != nil {
		return history, err
	}
	defer os.Remove(tmp.Name())
	if _, err := tmp.Write(append(data, '\n')); err != nil {
		tmp.Close()
		return history, err
	}
	if err := tmp.Close(); err != nil {
		return history, err
	}
	if err := os.Chmod(tmp.Name(), 0o600); err != nil {
		return history, err
	}
	if err := os.Rename(tmp.Name(), path); err != nil {
		return history, err
	}
	return merged, nil
}

// appendHistory records a prompt unless it repeats the previous one.
func appendHistory(h []historyEntry, prompt, id, model string) []historyEntry {
	if len(h) > 0 && h[len(h)-1].Prompt == prompt {
		return h
	}
	return append(h, historyEntry{Prompt: prompt, SessionID: id, Title: titleForPrompt(prompt), Model: model, CreatedAt: time.Now().UTC()})
}

func titleForPrompt(prompt string) string {
	prompt = strings.Join(strings.Fields(prompt), " ")
	r := []rune(prompt)
	if len(r) > 72 {
		return string(r[:71]) + "…"
	}
	return prompt
}
