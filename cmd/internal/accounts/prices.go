package accounts

import (
	"cmp"
	"encoding/json/v2"
	"errors"
	"fmt"
	"math"
	"os"
	"slices"
	"strings"
	"sync"
	"unicode/utf8"
)

// Prices turn tokens into what they would cost as API credits: what a
// provider's API charges for a model's tokens, by the kinds of token it
// prices apart. The cockpit knows the list prices of the models it knows
// (builtinPrices), and estimates those of newer models of a family it knows
// by the family's latest price; the user sets any other, or overrides any,
// with rules of their own — for one model, or for every model a pattern
// such as "gpt-6-*" matches — kept in prices.json beside the ledger.

// Price is what an API charges for a model's tokens, in US dollars per
// million tokens of each kind.
type Price struct {
	Input      float64 `json:"input"`
	Output     float64 `json:"output"`
	CacheRead  float64 `json:"cache_read"`
	CacheWrite float64 `json:"cache_write"`
}

// Cost is what each kind of token comes to, in US dollars.
type Cost struct {
	Input      float64 `json:"input"`
	CacheRead  float64 `json:"cache_read"`
	CacheWrite float64 `json:"cache_write"`
	Output     float64 `json:"output"`
}

// Total is the whole cost.
func (c Cost) Total() float64 { return c.Input + c.CacheRead + c.CacheWrite + c.Output }

func (c *Cost) add(o Cost) {
	c.Input += o.Input
	c.CacheRead += o.CacheRead
	c.CacheWrite += o.CacheWrite
	c.Output += o.Output
}

// Of is what tokens come to at the price. Tokens of no known kind count as
// input, the cheapest kind a provider charges for.
func (p Price) Of(t Tokens) Cost {
	const million = 1e6
	return Cost{
		Input:      float64(t.Input+t.Other) * p.Input / million,
		CacheRead:  float64(t.CacheRead) * p.CacheRead / million,
		CacheWrite: float64(t.CacheWrite) * p.CacheWrite / million,
		Output:     float64(t.Output) * p.Output / million,
	}
}

// Where a model's price comes from.
const (
	// PriceCustom is a price the user set.
	PriceCustom = "custom"
	// PriceList is the provider's list price, as the cockpit knows it.
	PriceList = "list"
	// PriceEstimate is the price of another model of the family, for a
	// model whose own price the cockpit does not know.
	PriceEstimate = "estimate"
	// PriceOpenRouter is the price OpenRouter lists, fetched.
	PriceOpenRouter = "openrouter"
	// PriceNone: the cockpit knows no price, and the user set none.
	PriceNone = "none"
)

// ModelPrice is the price a model's tokens are counted at, and where it
// comes from.
type ModelPrice struct {
	Price
	Source string `json:"source"`
	// Rule is the pattern that gave the price, such as "claude-opus-*".
	Rule string `json:"rule,omitzero"`
	// Like names the model whose price an estimate borrows; Note says
	// anything else worth knowing of the price.
	Like string `json:"like,omitzero"`
	Note string `json:"note,omitzero"`
}

// Priced reports whether the model has a price at all.
func (p ModelPrice) Priced() bool { return p.Source != PriceNone }

// PriceRule prices the models its pattern matches: a model's ID, or a
// pattern where * stands for anything, such as "gpt-6-*".
type PriceRule struct {
	Match string `json:"match"`
	Price
	Note string `json:"note,omitzero"`
}

// builtinPrice is a price the cockpit knows: for the models its patterns
// match, once dots are dashes (claude-opus-4.5 and claude-opus-4-5 alike).
type builtinPrice struct {
	match []string
	// name names the models of a list price for people.
	name  string
	price Price
	// like names the model an estimate borrows the price of; a price
	// without is the list price of the models it matches.
	like string
	note string
}

// Anthropic's cache reads cost a tenth of the input; its cache writes a
// quarter more than the input for five minutes and twice the input for an
// hour. The gateway's Claude requests ask for the hour, as Claude Code does.
func claudePrice(input, output float64) Price {
	return Price{Input: input, Output: output, CacheRead: round(input * 0.1), CacheWrite: round(input * 2)}
}

// Other providers charge nothing extra for writing a cache: what they
// report as written counts as input.
func priced(input, cacheRead, output float64) Price {
	return Price{Input: input, Output: output, CacheRead: cacheRead, CacheWrite: input}
}

const claudeNote = "Cache writes at the one-hour rate, twice the input, which the gateway's Claude requests ask for"

// builtinPrices are the list prices the cockpit knows, in US dollars per
// million tokens, and the estimates for the newer models of their families,
// the first match winning. Long-context and priority tiers are left out.
var builtinPrices = []builtinPrice{
	// Anthropic.
	{match: []string{"claude-opus-4-5*"}, name: "Claude Opus 4.5", price: claudePrice(5, 25), note: claudeNote},
	{match: []string{"claude-opus-4-1*", "claude-opus-4-0*", "claude-opus-4-2025*", "claude-opus-4", "claude-opus-4-thinking*", "claude-3-opus*"}, name: "Claude Opus 4 · 4.1", price: claudePrice(15, 75), note: claudeNote},
	{match: []string{"claude-sonnet-4-5*", "claude-sonnet-4-0*", "claude-sonnet-4-2025*", "claude-sonnet-4", "claude-sonnet-4-thinking*", "claude-3-7-sonnet*", "claude-3-5-sonnet*"}, name: "Claude Sonnet 4 · 4.5", price: claudePrice(3, 15), note: claudeNote},
	{match: []string{"claude-haiku-4-5*"}, name: "Claude Haiku 4.5", price: claudePrice(1, 5), note: claudeNote},
	{match: []string{"claude-3-5-haiku*"}, name: "Claude Haiku 3.5", price: claudePrice(0.8, 4), note: claudeNote},
	{match: []string{"claude-3-haiku*"}, name: "Claude Haiku 3", price: claudePrice(0.25, 1.25), note: claudeNote},
	{match: []string{"claude-opus-*"}, price: claudePrice(5, 25), like: "Claude Opus 4.5", note: claudeNote},
	{match: []string{"claude-sonnet-*"}, price: claudePrice(3, 15), like: "Claude Sonnet 4.5", note: claudeNote},
	{match: []string{"claude-haiku-*"}, price: claudePrice(1, 5), like: "Claude Haiku 4.5", note: claudeNote},

	// OpenAI.
	{match: []string{"gpt-5-pro*"}, name: "GPT-5 pro", price: priced(15, 15, 120)},
	{match: []string{"gpt-5-mini*", "gpt-5-codex-mini*", "gpt-5-1-codex-mini*"}, name: "GPT-5 mini", price: priced(0.25, 0.025, 2)},
	{match: []string{"gpt-5-nano*"}, name: "GPT-5 nano", price: priced(0.05, 0.005, 0.4)},
	{match: []string{"gpt-5", "gpt-5-2025*", "gpt-5-codex*", "gpt-5-chat*", "gpt-5-1", "gpt-5-1-2025*", "gpt-5-1-codex*", "gpt-5-1-chat*"}, name: "GPT-5 · 5.1", price: priced(1.25, 0.125, 10)},
	{match: []string{"gpt-4-1-mini*"}, name: "GPT-4.1 mini", price: priced(0.4, 0.1, 1.6)},
	{match: []string{"gpt-4-1-nano*"}, name: "GPT-4.1 nano", price: priced(0.1, 0.025, 0.4)},
	{match: []string{"gpt-4-1*"}, name: "GPT-4.1", price: priced(2, 0.5, 8)},
	{match: []string{"gpt-4o-mini*"}, name: "GPT-4o mini", price: priced(0.15, 0.075, 0.6)},
	{match: []string{"gpt-4o*"}, name: "GPT-4o", price: priced(2.5, 1.25, 10)},
	{match: []string{"o3-pro*"}, name: "o3-pro", price: priced(20, 20, 80)},
	{match: []string{"o3-mini*", "o1-mini*"}, name: "o3-mini", price: priced(1.1, 0.55, 4.4)},
	{match: []string{"o4-mini*"}, name: "o4-mini", price: priced(1.1, 0.275, 4.4)},
	{match: []string{"o3", "o3-2025*"}, name: "o3", price: priced(2, 0.5, 8)},
	{match: []string{"o1", "o1-2024*"}, name: "o1", price: priced(15, 7.5, 60)},
	{match: []string{"codex-mini*"}, name: "codex-mini", price: priced(1.5, 0.375, 6)},
	{match: []string{"gpt-5*-mini*"}, price: priced(0.25, 0.025, 2), like: "GPT-5 mini"},
	{match: []string{"gpt-5*-nano*"}, price: priced(0.05, 0.005, 0.4), like: "GPT-5 nano"},
	{match: []string{"gpt-5*"}, price: priced(1.25, 0.125, 10), like: "GPT-5"},

	// Google.
	{match: []string{"gemini-3-pro*"}, name: "Gemini 3 Pro", price: priced(2, 0.2, 12), note: "Prompts up to 200K tokens"},
	{match: []string{"gemini-2-5-pro*"}, name: "Gemini 2.5 Pro", price: priced(1.25, 0.125, 10), note: "Prompts up to 200K tokens"},
	{match: []string{"gemini-2-5-flash-lite*"}, name: "Gemini 2.5 Flash-Lite", price: priced(0.1, 0.01, 0.4)},
	{match: []string{"gemini-2-5-flash*"}, name: "Gemini 2.5 Flash", price: priced(0.3, 0.03, 2.5)},
	{match: []string{"gemini-2-0-flash-lite*"}, name: "Gemini 2.0 Flash-Lite", price: priced(0.075, 0.075, 0.3)},
	{match: []string{"gemini-2-0-flash*"}, name: "Gemini 2.0 Flash", price: priced(0.1, 0.025, 0.4)},
	{match: []string{"gemini-*-flash-lite*"}, price: priced(0.1, 0.01, 0.4), like: "Gemini 2.5 Flash-Lite"},
	{match: []string{"gemini-*-flash*"}, price: priced(0.3, 0.03, 2.5), like: "Gemini 2.5 Flash"},
	{match: []string{"gemini-*-pro*"}, price: priced(2, 0.2, 12), like: "Gemini 3 Pro", note: "Prompts up to 200K tokens"},

	// xAI.
	{match: []string{"grok-code-fast*"}, name: "Grok Code Fast 1", price: priced(0.2, 0.02, 1.5)},
	{match: []string{"grok-4-fast*", "grok-4-1-fast*"}, name: "Grok 4 Fast", price: priced(0.2, 0.05, 0.5)},
	{match: []string{"grok-4", "grok-4-0709", "grok-4-latest"}, name: "Grok 4", price: priced(3, 0.75, 15)},
	{match: []string{"grok-3-mini*"}, name: "Grok 3 Mini", price: priced(0.3, 0.075, 0.5)},
	{match: []string{"grok-3", "grok-3-latest", "grok-3-beta"}, name: "Grok 3", price: priced(3, 0.75, 15)},
	{match: []string{"grok-code*"}, price: priced(0.2, 0.02, 1.5), like: "Grok Code Fast 1"},
	{match: []string{"grok-4*fast*"}, price: priced(0.2, 0.05, 0.5), like: "Grok 4 Fast"},
	{match: []string{"grok-4*"}, price: priced(3, 0.75, 15), like: "Grok 4"},

	// Others that speak the Chat Completions API.
	{match: []string{"kimi-k2-turbo*", "kimi-k2-thinking-turbo*"}, name: "Kimi K2 Turbo", price: priced(1.15, 0.15, 8)},
	{match: []string{"kimi-k2*"}, name: "Kimi K2", price: priced(0.6, 0.15, 2.5)},
	{match: []string{"kimi-*"}, price: priced(0.6, 0.15, 2.5), like: "Kimi K2"},
	{match: []string{"deepseek-chat", "deepseek-reasoner", "deepseek-v3-2*"}, name: "DeepSeek V3.2", price: priced(0.28, 0.028, 0.42)},
	{match: []string{"glm-4-6*", "glm-4-5", "glm-4-5-2025*"}, name: "GLM-4.6", price: priced(0.6, 0.11, 2.2)},
	{match: []string{"glm-4-5-air*"}, name: "GLM-4.5-Air", price: priced(0.2, 0.03, 1.1)},
	{match: []string{"minimax-m2*"}, name: "MiniMax M2", price: priced(0.3, 0.03, 1.2)},
}

// round keeps prices at a thousandth of a cent per million tokens.
func round(v float64) float64 { return math.Round(v*1e5) / 1e5 }

// builtinKey is a model's ID as the built-in prices match it: lower case,
// without the path of a provider's catalog ("anthropic/…", "models/…"),
// Claude of Antigravity's under its own name, and dots as dashes.
func builtinKey(model string) string {
	key := strings.ToLower(strings.TrimSpace(model))
	if i := strings.LastIndex(key, "/"); i >= 0 {
		key = key[i+1:]
	}
	if rest, ok := strings.CutPrefix(key, "gemini-claude-"); ok {
		key = "claude-" + rest
	}
	return strings.ReplaceAll(key, ".", "-")
}

// builtinFor is the price the cockpit knows for a model.
func builtinFor(model string) (ModelPrice, bool) {
	key := builtinKey(model)
	for _, b := range builtinPrices {
		for _, pattern := range b.match {
			if !globMatch(pattern, key) {
				continue
			}
			source := PriceList
			if b.like != "" {
				source = PriceEstimate
			}
			return ModelPrice{Price: b.price, Source: source, Rule: pattern, Like: b.like, Note: b.note}, true
		}
	}
	return ModelPrice{}, false
}

// globMatch reports whether s matches a pattern where * stands for any run
// of characters, and nothing else is special.
func globMatch(pattern, s string) bool {
	parts := strings.Split(pattern, "*")
	if len(parts) == 1 {
		return pattern == s
	}
	if !strings.HasPrefix(s, parts[0]) {
		return false
	}
	s = s[len(parts[0]):]
	last := parts[len(parts)-1]
	for _, part := range parts[1 : len(parts)-1] {
		i := strings.Index(s, part)
		if i < 0 {
			return false
		}
		s = s[i+len(part):]
	}
	return strings.HasSuffix(s, last) && len(s) >= len(last)
}

// Unit is what amounts are shown in: US dollars, or credits of the user's
// that are worth USD dollars each.
type Unit struct {
	Name string  `json:"name"`
	USD  float64 `json:"usd"`
}

// Dollars is the unit of API prices.
var Dollars = Unit{Name: "USD", USD: 1}

// PriceBook holds the user's price rules and unit, kept in a file.
type PriceBook struct {
	path string
	// market is the prices fetched from OpenRouter, if any.
	market *OpenRouter

	mu    sync.Mutex
	rules []PriceRule
	unit  Unit
	err   error // why the file could not be read, if it could not
}

type priceFile struct {
	Version int         `json:"version"`
	Unit    *Unit       `json:"unit,omitzero"`
	Rules   []PriceRule `json:"rules"`
}

// NewPriceBook reads the prices kept at path; "" keeps them in memory.
func NewPriceBook(path string) *PriceBook {
	b := &PriceBook{path: path, unit: Dollars}
	if path == "" {
		return b
	}
	data, err := os.ReadFile(path)
	if errors.Is(err, os.ErrNotExist) {
		return b
	}
	var saved priceFile
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
	for _, r := range saved.Rules {
		if r, err := cleanRule(r); err == nil {
			b.rules = append(b.rules, r)
		}
	}
	if saved.Unit != nil && validUnit(*saved.Unit) == nil {
		b.unit = *saved.Unit
	}
	return b
}

// Resolve is the price a model's tokens are counted at: the user's rule for
// the model, else the user's most specific pattern that matches it, else
// the price the cockpit knows.
func (b *PriceBook) Resolve(model string) ModelPrice {
	b.mu.Lock()
	rules := b.rules
	b.mu.Unlock()
	return resolvePrice(rules, b.market, model)
}

// resolvePrice prices a model by the user's rules, else OpenRouter's list,
// else the prices built in.
func resolvePrice(rules []PriceRule, market *OpenRouter, model string) ModelPrice {
	key := strings.ToLower(strings.TrimSpace(model))
	var best *PriceRule
	for i := range rules {
		r := &rules[i]
		if !globMatch(r.Match, key) {
			continue
		}
		if best == nil || specificity(r.Match) > specificity(best.Match) {
			best = r
		}
	}
	if best != nil {
		return ModelPrice{Price: best.Price, Source: PriceCustom, Rule: best.Match, Note: best.Note}
	}
	if listed, ok := market.Lookup(model); ok && key != "" {
		return listed
	}
	if known, ok := builtinFor(model); ok {
		return known
	}
	return ModelPrice{Source: PriceNone}
}

// specificity ranks patterns: a model's own ID over any pattern, and a
// pattern over another by the characters it names.
func specificity(pattern string) int {
	if !strings.Contains(pattern, "*") {
		return math.MaxInt
	}
	return len(pattern) - strings.Count(pattern, "*")
}

// Rules are the user's rules, the most specific first.
func (b *PriceBook) Rules() []PriceRule {
	b.mu.Lock()
	defer b.mu.Unlock()
	out := slices.Clone(b.rules)
	slices.SortStableFunc(out, func(x, y PriceRule) int {
		return cmp.Or(cmp.Compare(specificity(y.Match), specificity(x.Match)), strings.Compare(x.Match, y.Match))
	})
	return out
}

// Unit is what amounts are shown in.
func (b *PriceBook) Unit() Unit {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.unit
}

// Err says why the prices kept could not be read; saving replaces them.
func (b *PriceBook) Err() error {
	b.mu.Lock()
	defer b.mu.Unlock()
	return b.err
}

// maxRules bounds the rules a user keeps.
const maxRules = 500

// Set adds a rule, or replaces the rule of the same pattern.
func (b *PriceBook) Set(rule PriceRule) (PriceRule, error) {
	rule, err := cleanRule(rule)
	if err != nil {
		return PriceRule{}, err
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	rules := slices.Clone(b.rules)
	if i := slices.IndexFunc(rules, func(r PriceRule) bool { return r.Match == rule.Match }); i >= 0 {
		rules[i] = rule
	} else if len(rules) >= maxRules {
		return PriceRule{}, fmt.Errorf("at most %d prices", maxRules)
	} else {
		rules = append(rules, rule)
	}
	if err := b.save(rules, b.unit); err != nil {
		return PriceRule{}, err
	}
	b.rules = rules
	return rule, nil
}

// Remove drops the rule of a pattern: its models go back to the prices the
// cockpit knows.
func (b *PriceBook) Remove(match string) error {
	match = strings.ToLower(strings.TrimSpace(match))
	b.mu.Lock()
	defer b.mu.Unlock()
	rules := slices.DeleteFunc(slices.Clone(b.rules), func(r PriceRule) bool { return r.Match == match })
	if len(rules) == len(b.rules) {
		return fmt.Errorf("no price of yours for %q", match)
	}
	if err := b.save(rules, b.unit); err != nil {
		return err
	}
	b.rules = rules
	return nil
}

// SetUnit chooses what amounts are shown in.
func (b *PriceBook) SetUnit(unit Unit) (Unit, error) {
	unit.Name = strings.TrimSpace(unit.Name)
	if strings.EqualFold(unit.Name, "usd") || unit.Name == "" || unit.Name == "$" {
		unit = Dollars
	}
	if err := validUnit(unit); err != nil {
		return Unit{}, err
	}
	b.mu.Lock()
	defer b.mu.Unlock()
	if err := b.save(b.rules, unit); err != nil {
		return Unit{}, err
	}
	b.unit = unit
	return unit, nil
}

func validUnit(u Unit) error {
	if u.Name == "" || utf8.RuneCountInString(u.Name) > 24 || strings.ContainsAny(u.Name, "\n\r\t") {
		return errors.New("a unit has a name of up to 24 characters")
	}
	if !(u.USD > 0) || u.USD > 1e6 || math.IsInf(u.USD, 0) {
		return errors.New("a unit is worth more than nothing, in dollars")
	}
	return nil
}

// save writes rules and unit; the caller holds b.mu.
func (b *PriceBook) save(rules []PriceRule, unit Unit) error {
	if b.path == "" {
		return nil
	}
	file := priceFile{Version: 1, Rules: rules}
	if unit != Dollars {
		file.Unit = &unit
	}
	if file.Rules == nil {
		file.Rules = []PriceRule{}
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

// cleanRule checks a rule: a pattern of a model's ID, and prices a
// provider could charge.
func cleanRule(r PriceRule) (PriceRule, error) {
	r.Match = strings.ToLower(strings.TrimSpace(r.Match))
	r.Note = strings.TrimSpace(r.Note)
	switch {
	case r.Match == "" || strings.Trim(r.Match, "*") == "":
		return r, errors.New("a price names a model, or a pattern such as gpt-6-*")
	case len(r.Match) > 200 || strings.ContainsAny(r.Match, " \t\r\n"):
		return r, errors.New("a model's ID has no spaces")
	case utf8.RuneCountInString(r.Note) > 200:
		return r, errors.New("a note takes up to 200 characters")
	}
	for _, v := range []float64{r.Input, r.Output, r.CacheRead, r.CacheWrite} {
		if v < 0 || v > 10000 || math.IsNaN(v) || math.IsInf(v, 0) {
			return r, errors.New("prices are dollars per million tokens, from 0 to 10000")
		}
	}
	r.Input, r.Output, r.CacheRead, r.CacheWrite = round(r.Input), round(r.Output), round(r.CacheRead), round(r.CacheWrite)
	return r, nil
}

// BuiltinPrice is a price the cockpit knows, as the price editor offers it
// to borrow.
type BuiltinPrice struct {
	Match []string `json:"match"`
	Name  string   `json:"name,omitzero"`
	Price
	Source string `json:"source"`
	Like   string `json:"like,omitzero"`
	Note   string `json:"note,omitzero"`
}

// BuiltinPrices lists the prices the cockpit knows.
func BuiltinPrices() []BuiltinPrice {
	out := make([]BuiltinPrice, 0, len(builtinPrices))
	for _, b := range builtinPrices {
		source := PriceList
		if b.like != "" {
			source = PriceEstimate
		}
		out = append(out, BuiltinPrice{Match: slices.Clone(b.match), Name: b.name, Price: b.price, Source: source, Like: b.like, Note: b.note})
	}
	return out
}
