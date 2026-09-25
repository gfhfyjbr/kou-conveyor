package main

import (
	"errors"
	"fmt"
	"slices"
	"strings"

	"github.com/charmbracelet/bubbles/textinput"
	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/lipgloss"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

var formAPIs = []struct {
	api         cockpit.API
	name, gloss string
}{
	{cockpit.APIGateway, "GATEWAY", "accounts gateway"},
	{cockpit.APIEnvironment, "ENVIRONMENT", "runner defaults"},
	{cockpit.APIResponses, "RESPONSES", "OpenAI API"},
	{cockpit.APIMessages, "MESSAGES", "Anthropic API"},
}

// Form rows: the provider type, then one row per field.
const (
	rowAPI = iota
	rowBase
	rowKey
	rowModel
)

// settingsForm edits the connection both cockpits hand to the runner. It
// never shows the saved key: an empty key field keeps it.
type settingsForm struct {
	path     string
	saved    cockpit.Settings
	api      int
	fields   [3]textinput.Model // base URL, key, model
	focus    int
	clearKey bool
	status   string
	level    string // info, ok, warn or error
	checking bool
	checks   int // a check reports only if nothing it checked has changed
	models   []string
}

type checkMsg struct {
	gen, seq int
	check    cockpit.Check
}

func newSettingsForm(st styles, path string, saved cockpit.Settings) *settingsForm {
	f := &settingsForm{path: path, saved: saved}
	for i := range f.fields {
		in := textinput.New()
		in.Prompt = ""
		in.CharLimit = 4096
		in.TextStyle, in.PlaceholderStyle, in.Cursor.Style = st.text, st.faint, st.accent
		f.fields[i] = in
	}
	f.fields[1].EchoMode = textinput.EchoPassword
	f.fields[1].EchoCharacter = '•'
	for i, a := range formAPIs {
		if a.api == saved.API {
			f.api = i
		}
	}
	f.fields[0].SetValue(saved.BaseURL)
	f.fields[2].SetValue(saved.Model)
	f.placeholders()
	return f
}

func (f *settingsForm) selected() cockpit.API { return formAPIs[f.api].api }

// sameEndpoint reports whether the form still points where the saved key
// belongs, which is the only place it is ever sent.
func (f *settingsForm) sameEndpoint() bool {
	defaults := cockpit.DefaultsFor(f.selected())
	base := func(s string) string {
		s = strings.TrimRight(strings.TrimSpace(s), "/")
		if s == "" {
			s = defaults.BaseURL
		}
		return strings.ToLower(s)
	}
	return f.selected() == f.saved.API && base(f.fields[0].Value()) == base(f.saved.BaseURL)
}

func (f *settingsForm) placeholders() {
	if f.selected() == cockpit.APIGateway {
		f.fields[2].Placeholder = "a model the gateway serves; ^T lists them"
		return
	}
	defaults := cockpit.DefaultsFor(f.selected())
	f.fields[0].Placeholder = defaults.BaseURL
	f.fields[2].Placeholder = defaults.Model
	if defaults.Model != "" {
		f.fields[2].Placeholder += " (default)"
	}
	switch {
	case f.saved.APIKey != "" && f.sameEndpoint() && !f.clearKey:
		f.fields[1].Placeholder = "saved " + f.saved.KeyHint() + " · empty keeps it · ctrl+d removes it"
	case f.saved.APIKey != "" && !f.sameEndpoint():
		f.fields[1].Placeholder = "enter the key for this endpoint"
	case f.clearKey:
		f.fields[1].Placeholder = "the saved key is removed on save"
	default:
		f.fields[1].Placeholder = "paste a key, or leave empty to use " + defaults.KeyVariable
	}
}

func (f *settingsForm) update() cockpit.SettingsUpdate {
	if f.selected() == cockpit.APIGateway {
		u := cockpit.SettingsUpdate{API: cockpit.APIGateway, Model: f.fields[2].Value()}
		// The gateway's protocol, chosen in the browser, stays with its model.
		if f.saved.API == cockpit.APIGateway && strings.TrimSpace(f.fields[2].Value()) == f.saved.Model {
			u.Protocol = f.saved.Protocol
		}
		return u
	}
	u := cockpit.SettingsUpdate{API: f.selected(), BaseURL: f.fields[0].Value(), Model: f.fields[2].Value()}
	if key := strings.TrimSpace(f.fields[1].Value()); key != "" {
		u.APIKey = &key
	} else if f.clearKey {
		u.APIKey = new("")
	}
	return u
}

// visible are the rows the selected API has.
func (f *settingsForm) visible() []int {
	switch f.selected() {
	case cockpit.APIEnvironment:
		return []int{rowAPI}
	case cockpit.APIGateway:
		return []int{rowAPI, rowModel}
	}
	return []int{rowAPI, rowBase, rowKey, rowModel}
}

// step moves the focus to the next or the previous row the API has.
func (f *settingsForm) step(delta int) {
	rows := f.visible()
	at := max(0, slices.Index(rows, f.focus))
	f.setFocus(rows[(at+delta+len(rows))%len(rows)])
}

func (f *settingsForm) setFocus(row int) {
	if !slices.Contains(f.visible(), row) {
		row = rowAPI
	}
	f.focus = row
	for i := range f.fields {
		if i+1 == f.focus {
			f.fields[i].Focus()
		} else {
			f.fields[i].Blur()
		}
	}
}

func (f *settingsForm) say(text, level string) { f.status, f.level = text, level }

// openSettings shows the settings form over the transcript.
func (m *uiModel) openSettings() tea.Cmd {
	if m.opt.SettingsFile == "" {
		return m.notify("no settings file: set KOU_CONVEYOR_CONFIG", "error")
	}
	saved, err := cockpit.LoadSettings(m.opt.SettingsFile)
	m.closePicker()
	m.changes.focused = false
	m.form = newSettingsForm(m.styles, m.opt.SettingsFile, saved)
	m.formGen++
	m.input.Blur()
	switch {
	case err != nil:
		// The form still opens, so the settings can be fixed from it.
		m.form.say(err.Error()+"; saving replaces them", "error")
	case m.opt.Provider != "":
		m.form.say("-provider "+m.opt.Provider+" takes precedence over these settings while it is set", "warn")
	}
	return nil
}

func (m *uiModel) closeForm() {
	m.form = nil
	m.formGen++
	m.input.Focus()
}

func (m *uiModel) formKey(msg tea.KeyMsg) tea.Cmd {
	f := m.form
	switch msg.String() {
	case "esc", "ctrl+c":
		m.closeForm()
		return nil
	case "tab", "down":
		f.step(1)
		return nil
	case "shift+tab", "up":
		f.step(-1)
		return nil
	case "enter":
		return m.saveSettings()
	case "ctrl+t":
		return m.checkSettings()
	case "ctrl+d":
		if f.focus == rowKey && f.saved.APIKey != "" && f.sameEndpoint() {
			f.clearKey = !f.clearKey
			f.fields[1].SetValue("")
			f.placeholders()
			return nil
		}
	case "left", "right":
		if f.focus == rowAPI {
			step := 1
			if msg.String() == "left" {
				step = len(formAPIs) - 1
			}
			f.api = (f.api + step) % len(formAPIs)
			f.models, f.status, f.checking = nil, "", false
			f.checks++
			// A base URL and model belong to their API: the saved ones come
			// back with it, and another API starts from its defaults.
			base, model := "", ""
			if f.selected() == f.saved.API {
				base, model = f.saved.BaseURL, f.saved.Model
			}
			f.fields[0].SetValue(base)
			f.fields[2].SetValue(model)
			f.placeholders()
			return nil
		}
	}
	if f.focus == rowAPI {
		return nil
	}
	var cmd tea.Cmd
	before := f.fields[f.focus-1].Value()
	f.fields[f.focus-1], cmd = f.fields[f.focus-1].Update(msg)
	if f.fields[f.focus-1].Value() != before {
		// A check answers for what was checked; the form changed since.
		f.checks++
		if f.checking {
			f.checking = false
			f.say("", "")
		}
	}
	if f.focus == rowBase {
		f.placeholders()
	}
	return cmd
}

func (m *uiModel) saveSettings() tea.Cmd {
	f := m.form
	next, err := f.saved.Update(f.update())
	if errors.Is(err, cockpit.ErrKeyRequired) {
		f.say("enter the API key again: the saved key belongs to a different endpoint", "error")
		f.setFocus(rowKey)
		return nil
	}
	if err == nil {
		err = cockpit.SaveSettings(f.path, next)
	}
	if err != nil {
		f.say(err.Error(), "error")
		return nil
	}
	m.closeForm()
	m.resolveConnection(next)
	return m.notify("connection saved · "+m.connectionSummary()+" · applies to the next run", "info")
}

func (m *uiModel) checkSettings() tea.Cmd {
	f := m.form
	draft, err := f.saved.Update(f.update())
	if errors.Is(err, cockpit.ErrKeyRequired) {
		f.say("enter the API key for this endpoint", "error")
		return nil
	}
	if err != nil {
		f.say(err.Error(), "error")
		return nil
	}
	f.checking = true
	f.checks++
	f.say("checking…", "info")
	gen, seq, ctx, settingsFile := m.formGen, f.checks, m.ctx, m.opt.SettingsFile
	workspace := m.opt.Workspace
	return func() tea.Msg {
		// Through the gateway, the check lists what the gateway serves.
		if draft.API == cockpit.APIGateway {
			resolved, err := cockpit.ResolveGateway(ctx, settingsFile, draft)
			if err != nil {
				return checkMsg{gen: gen, seq: seq, check: cockpit.Check{Message: err.Error()}}
			}
			draft = resolved
		}
		connection := cockpit.ResolveConnection("", draft, cockpit.WorkspaceEnv(workspace))
		return checkMsg{gen: gen, seq: seq, check: cockpit.CheckConnection(ctx, connection)}
	}
}

func (m *uiModel) checked(msg checkMsg) {
	if m.form == nil || msg.gen != m.formGen || msg.seq != m.form.checks {
		return
	}
	f := m.form
	f.checking = false
	level := "error"
	if msg.check.OK {
		level = "ok"
	}
	f.say(msg.check.Message, level)
	f.models = msg.check.Models
}

// resolveConnection works out what the next run will use, for display.
func (m *uiModel) resolveConnection(settings cockpit.Settings) {
	m.conn = cockpit.ResolveConnection(m.opt.Provider, settings, cockpit.WorkspaceEnv(m.opt.Workspace))
}

func (m *uiModel) connectionSummary() string {
	model := orDefault(m.opt.model, m.conn.Model)
	provider := m.conn.Provider
	if m.conn.Source == "gateway" {
		provider = "gateway"
	}
	parts := []string{orDefault(provider, "runner default")}
	if model != "" {
		parts = append(parts, model)
	}
	return strings.Join(parts, " · ")
}

func (f *settingsForm) view(st styles, width, height int) string {
	inner := max(24, width-4)
	label := func(row int, text string) string {
		style := st.label
		if f.focus == row {
			style = st.accentLabel
		}
		return style.Render(fmt.Sprintf("%-10s", text)) + "  "
	}
	var b strings.Builder
	b.WriteString(fitRight(st.accentLabel.Render("CONNECTION"), st.faint.Render(ansi.Truncate(f.path, max(8, inner-14), "…")), inner) + "\n")
	b.WriteString(st.rule.Render(strings.Repeat("─", inner)) + "\n")

	var choices []string
	for i, a := range formAPIs {
		name := " " + a.name + " "
		switch {
		case i == f.api && f.focus == rowAPI:
			choices = append(choices, st.selected.Foreground(colorAccent).Render("‹"+name+"›"))
		case i == f.api:
			choices = append(choices, st.accentLabel.Render("‹"+name+"›"))
		default:
			choices = append(choices, st.faint.Render(" "+name+" "))
		}
	}
	b.WriteString(label(rowAPI, "API") + strings.Join(choices, "") + "\n")
	b.WriteString(strings.Repeat(" ", 12) + st.faint.Render(formAPIs[f.api].gloss) + "\n\n")

	switch f.selected() {
	case cockpit.APIEnvironment:
		note := "Runs use the environment: KOU_CONVEYOR_LLM_PROVIDER, _BASE_URL, _API_KEY and _MODEL, or the provider's own key variable such as OPENAI_API_KEY."
		b.WriteString(strings.Join(wrapText(note, st.muted, inner, "", ""), "\n") + "\n")
	case cockpit.APIGateway:
		note := "Runs go through the accounts gateway that kou-conveyor-web runs, to its accounts and endpoints, over the API the model speaks. "
		if g, err := cockpit.LoadGateway(cockpit.GatewayPath(f.path)); err == nil {
			note += "It runs at " + g.URL + "."
		} else {
			note += "It is not running now: start kou-conveyor-web."
		}
		b.WriteString(strings.Join(wrapText(note, st.muted, inner, "", ""), "\n") + "\n")
		f.fields[2].Width = max(8, inner-13)
		b.WriteString(label(rowModel, "MODEL") + f.fields[2].View() + "\n")
		b.WriteString(f.modelHints(st, inner))
	default:
		for i, name := range []string{"BASE URL", "API KEY", "MODEL"} {
			f.fields[i].Width = max(8, inner-13)
			b.WriteString(label(i+1, name) + f.fields[i].View() + "\n")
		}
		b.WriteString(f.modelHints(st, inner))
	}
	if f.status != "" {
		glyph, style := "›", st.muted
		switch f.level {
		case "ok":
			glyph, style = "✓", st.ok
		case "warn":
			glyph, style = "!", st.warn
		case "error":
			glyph, style = "✗", st.err
		}
		b.WriteString("\n" + strings.Join(wrapText(glyph+" "+f.status, style, inner, "", "  "), "\n") + "\n")
	}
	b.WriteString(st.rule.Render(strings.Repeat("─", inner)) + "\n")
	hints := []string{"tab", "next", "enter", "save", "^T", "check", "esc", "cancel"}
	if f.focus == rowAPI {
		hints = append([]string{"←→", "API"}, hints...)
	}
	b.WriteString(keyHints(st, inner, hints...))
	box := st.box.Width(inner + 2).Render(b.String())
	if lines := strings.Split(box, "\n"); len(lines) > height {
		box = strings.Join(lines[:height], "\n")
	}
	return box
}

// modelHints lists checked models that start with what the model field holds.
func (f *settingsForm) modelHints(st styles, inner int) string {
	if len(f.models) == 0 {
		return ""
	}
	prefix := strings.TrimSpace(f.fields[2].Value())
	var shown []string
	for _, model := range f.models {
		if strings.HasPrefix(model, prefix) && len(shown) < 6 {
			shown = append(shown, model)
		}
	}
	if len(shown) == 0 {
		return ""
	}
	return strings.Repeat(" ", 12) + st.faint.Render(ansi.Truncate(strings.Join(shown, "  "), inner-12, "…")) + "\n"
}

// formOverlay centers the settings form over the transcript area.
func (m *uiModel) formOverlay() string {
	height := m.view.Height
	box := m.form.view(m.styles, min(84, m.width-2), height)
	lines := strings.Split(box, "\n")
	left := max(0, (m.width-lipgloss.Width(lines[0]))/2)
	top := 0
	if height-len(lines) >= 2 {
		top = 1
	}
	out := make([]string, height)
	for i := range out {
		if j := i - top; j >= 0 && j < len(lines) {
			out[i] = strings.Repeat(" ", left) + lines[j]
		}
	}
	return strings.Join(out, "\n")
}
