package main

import (
	"io"
	"os"
	"path/filepath"
	"strings"
	"testing"
	"time"

	tea "github.com/charmbracelet/bubbletea"
	"github.com/charmbracelet/x/ansi"
	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

func runPrompt(t *testing.T, m *uiModel, prompt string) {
	t.Helper()
	m.input.SetValue(prompt)
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.state == idle })
}

func typeText(m *uiModel, text string) {
	m.Update(tea.KeyMsg{Type: tea.KeyRunes, Runes: []rune(text)})
}

func TestSessionCommands(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "first question")
	runPrompt(t, m, "second question")
	parent := m.sessionID

	m.command("/rename Release prep")
	m.command("/pin")
	header := ansi.Strip(m.header())
	if m.sessionTitle() != "Release prep" || !m.meta.Pinned || !strings.Contains(header, "◆ Release prep") {
		t.Fatalf("title %q, pinned %v, header %q", m.sessionTitle(), m.meta.Pinned, header)
	}
	if list, _ := cockpit.ListSessions(m.opt.SessionDir); len(list) != 1 || list[0].Title != "Release prep" || !list[0].Pinned {
		t.Fatalf("sessions = %+v", list)
	}
	m.command("/rename")
	if m.input.Value() != "/rename Release prep" {
		t.Fatalf("composer = %q", m.input.Value())
	}
	m.input.Reset()

	m.command("/export")
	m.command("/export")
	for _, name := range []string{"release-prep.md", "release-prep-2.md"} {
		data, err := os.ReadFile(filepath.Join(m.opt.Workspace, name))
		if err != nil || !strings.Contains(string(data), "# Release prep") || !strings.Contains(string(data), "echo: second question") {
			t.Fatalf("%s: %v\n%s", name, err, data)
		}
	}

	if m.command("/fork 9"); m.note.level != "warn" {
		t.Fatalf("fork of a missing prompt: %+v", m.note)
	}
	drive(t, m, m.command("/fork 2"), func() bool { return m.sessionID != parent && !m.loading })
	if m.input.Value() != "second question" || kinds(m.tr) != "user,assistant,notice" || m.sessionTitle() != "Release prep · branch" {
		t.Fatalf("branch: composer %q, transcript %s, title %q", m.input.Value(), kinds(m.tr), m.sessionTitle())
	}
	branch := m.sessionID
	m.input.Reset()

	m.command("/delete")
	if m.picker == nil || m.picker.kind != "confirm" || m.picker.selected().title != "Keep the session" {
		t.Fatalf("delete did not ask first: %+v", m.picker)
	}
	m.Update(key("down"))
	m.Update(key("enter"))
	if !m.fresh || m.sessionID == branch {
		t.Fatalf("still on the deleted session %q", m.sessionID)
	}
	if _, err := os.Stat(cockpit.SessionPath(m.opt.SessionDir, branch)); !os.IsNotExist(err) {
		t.Fatalf("the branch survived: %v", err)
	}
	if m.command("/rename x"); m.note.level != "warn" {
		t.Fatal("renamed a session the runner never saved")
	}
}

func TestContinueAfterInterruptedRun(t *testing.T) {
	m := testModel(t)
	m.input.SetValue("wait for a signal")
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return len(m.tr.Entries) > 0 && m.tr.Entries[0].State == "" })
	press(m, key("esc"))
	stop := press(m, key("esc"))
	drive(t, m, tea.Batch(stop, waitJob(m.jobGen, m.job)), func() bool { return m.state == idle })
	m.note = notice{} // notices expire on a timer the test does not run
	if !m.tr.Interrupted() || !strings.Contains(ansi.Strip(m.statusLine()), "interrupted") {
		t.Fatalf("interrupted %v, status %q", m.tr.Interrupted(), ansi.Strip(m.statusLine()))
	}

	m.input.SetValue("a draft to keep")
	drive(t, m, m.command("/continue"), func() bool { return m.state == idle && len(m.tr.Entries) > 3 })
	last := m.tr.Entries[len(m.tr.Entries)-1]
	if last.Text != "echo: "+cockpit.ContinuePrompt || m.tr.Interrupted() || m.input.Value() != "a draft to keep" {
		t.Fatalf("after continue: %s, last %q, composer %q", kinds(m.tr), last.Text, m.input.Value())
	}
	if m.command("/continue"); m.note.level != "info" || m.job != nil {
		t.Fatal("continued a finished run")
	}
}

func TestSessionsPickerActions(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "alpha")
	alpha := m.sessionID
	m.newSession()
	runPrompt(t, m, "beta")

	open := func() {
		t.Helper()
		m.closePicker()
		drive(t, m, m.openPicker("sessions"), func() bool { return m.picker != nil && !m.picker.loading })
		m.picker.selectID(alpha)
	}
	open()
	m.Update(key("ctrl+e"))
	if m.picker.editing != alpha || m.picker.query.Value() != "alpha" {
		t.Fatalf("editing %q with %q", m.picker.editing, m.picker.query.Value())
	}
	m.picker.query.SetValue("")
	typeText(m, "Alpha, renamed")
	_, cmd := m.Update(key("enter"))
	drive(t, m, cmd, func() bool { return m.picker.selected() != nil && m.picker.selected().title == "Alpha, renamed" })

	_, cmd = m.Update(key("ctrl+t"))
	drive(t, m, cmd, func() bool { return m.picker.items[0].pinned })
	if m.picker.items[0].id != alpha || m.picker.selected().id != alpha {
		t.Fatalf("pinned session is not first: %+v", m.picker.items)
	}

	m.Update(key("ctrl+d"))
	if m.picker.armed != alpha || !strings.Contains(ansi.Strip(m.View()), "ctrl+d again deletes") {
		t.Fatal("the first ctrl+d did not ask again")
	}
	m.Update(key("down")) // any other key disarms
	m.picker.selectID(alpha)
	if m.Update(key("ctrl+d")); m.picker.armed != alpha {
		t.Fatal("did not re-arm")
	}
	_, cmd = m.Update(key("ctrl+d"))
	drive(t, m, cmd, func() bool { return len(m.picker.items) == 1 })
	if _, err := os.Stat(cockpit.SessionPath(m.opt.SessionDir, alpha)); !os.IsNotExist(err) {
		t.Fatalf("alpha survived: %v", err)
	}

	open()
	_, cmd = m.Update(key("ctrl+b"))
	drive(t, m, cmd, func() bool { return m.picker == nil && !m.loading && strings.Contains(m.sessionTitle(), "branch") })
	if kinds(m.tr) != "user,assistant,notice" {
		t.Fatalf("duplicate = %s", kinds(m.tr))
	}
}

func TestSettingsForm(t *testing.T) {
	m := testModel(t)
	m.opt.SettingsFile = filepath.Join(t.TempDir(), "settings.json")
	m.command("/settings")
	if m.form == nil {
		t.Fatalf("no form: %+v", m.note)
	}
	m.Update(key("right"))
	m.Update(key("right"))
	m.Update(key("tab")) // base URL: the default
	m.Update(key("tab"))
	typeText(m, "sk-ant-secret-1234")
	m.Update(key("tab"))
	typeText(m, "claude-sonnet-5")
	if view := ansi.Strip(m.View()); strings.Contains(view, "sk-ant-secret") || !strings.Contains(view, "MESSAGES") {
		t.Fatalf("form shows the key or the wrong API:\n%s", view)
	}
	m.Update(key("enter"))
	saved, err := cockpit.LoadSettings(m.opt.SettingsFile)
	if err != nil || m.form != nil || saved != (cockpit.Settings{API: cockpit.APIMessages, APIKey: "sk-ant-secret-1234", Model: "claude-sonnet-5"}) {
		t.Fatalf("saved %+v, %v, form open %v", saved, err, m.form != nil)
	}
	m.note = notice{}
	if status := ansi.Strip(m.statusLine()); !strings.Contains(status, "anthropic · claude-sonnet-5") {
		t.Fatalf("status = %q", status)
	}

	// Another API starts from its own defaults; the saved values come back.
	m.command("/settings")
	m.Update(key("left"))
	if m.form.fields[2].Value() != "" || !strings.Contains(m.form.fields[2].Placeholder, "gpt-") {
		t.Fatalf("responses kept the Messages model: %q (%q)", m.form.fields[2].Value(), m.form.fields[2].Placeholder)
	}
	m.Update(key("right"))
	if m.form.fields[2].Value() != "claude-sonnet-5" {
		t.Fatalf("the saved model did not come back: %q", m.form.fields[2].Value())
	}
	press(m, key("esc"))

	// The saved key only ever goes to its own endpoint.
	m.command("/settings")
	m.Update(key("tab"))
	typeText(m, "https://collector.example.com")
	m.Update(key("enter"))
	if m.form == nil || m.form.level != "error" || m.form.focus != rowKey {
		t.Fatalf("form after a moved endpoint: %+v", m.form)
	}
	if view := ansi.Strip(m.View()); strings.Contains(view, "sk-ant-secret") || !strings.Contains(view, "enter the API key again") {
		t.Fatalf("form view:\n%s", view)
	}
	press(m, key("esc"))
	if m.form != nil {
		t.Fatal("esc kept the form open")
	}
	if saved, _ := cockpit.LoadSettings(m.opt.SettingsFile); saved.BaseURL != "" {
		t.Fatalf("a cancelled edit was saved: %+v", saved)
	}
	for _, size := range [][2]int{{40, 12}, {80, 24}} {
		m.command("/settings")
		m.Update(tea.WindowSizeMsg{Width: size[0], Height: size[1]})
		lines := strings.Split(m.View(), "\n")
		if len(lines) != size[1] {
			t.Fatalf("%v: %d lines", size, len(lines))
		}
		for _, line := range lines {
			if ansi.StringWidth(line) > size[0] {
				t.Fatalf("%v: line too wide: %q", size, ansi.Strip(line))
			}
		}
		press(m, key("esc"))
	}
}

func TestSettingsFormGateway(t *testing.T) {
	m := testModel(t)
	m.opt.SettingsFile = filepath.Join(t.TempDir(), "settings.json")
	if err := cockpit.SaveSettings(m.opt.SettingsFile, cockpit.Settings{API: cockpit.APIMessages, APIKey: "sk-ant-secret-1234", Model: "claude-sonnet-5"}); err != nil {
		t.Fatal(err)
	}
	m.command("/settings")
	for m.form.selected() != cockpit.APIGateway {
		m.Update(key("left"))
	}
	if view := ansi.Strip(m.View()); !strings.Contains(view, "GATEWAY") || !strings.Contains(view, "not running") || strings.Contains(view, "BASE URL") {
		t.Fatalf("gateway form:\n%s", view)
	}
	m.Update(key("tab"))
	if m.form.focus != rowModel {
		t.Fatalf("focus = %d", m.form.focus)
	}
	typeText(m, "claude-opus-5-5")
	m.Update(key("tab")) // back to the API: the gateway has no other row
	if m.form.focus != rowAPI {
		t.Fatalf("focus after the model = %d", m.form.focus)
	}
	m.Update(key("enter"))
	saved, err := cockpit.LoadSettings(m.opt.SettingsFile)
	if err != nil || saved != (cockpit.Settings{API: cockpit.APIGateway, Model: "claude-opus-5-5"}) {
		t.Fatalf("saved %+v, %v", saved, err)
	}
	m.note = notice{}
	if status := ansi.Strip(m.statusLine()); !strings.Contains(status, "gateway · claude-opus-5-5") {
		t.Fatalf("status = %q", status)
	}
	// A published gateway shows where it runs.
	if err := cockpit.PublishGateway(cockpit.GatewayPath(m.opt.SettingsFile), cockpit.Gateway{URL: "http://127.0.0.1:8318", APIKey: "sk-ua-k"}); err != nil {
		t.Fatal(err)
	}
	m.command("/settings")
	if view := ansi.Strip(m.View()); !strings.Contains(view, "http://127.0.0.1:8318") || strings.Contains(view, "sk-ua-k") {
		t.Fatalf("gateway form:\n%s", view)
	}
	press(m, key("esc"))
}

func TestParseOptionsSessionFlags(t *testing.T) {
	workspace := t.TempDir()
	sessions := filepath.Join(workspace, ".harness", "sessions")
	os.MkdirAll(sessions, 0o700)
	now := time.Now()
	for id, age := range map[string]time.Duration{"abc-1111": time.Hour, "abd-2222": time.Minute} {
		path := cockpit.SessionPath(sessions, id)
		os.WriteFile(path, nil, 0o600)
		os.Chtimes(path, now.Add(-age), now.Add(-age))
	}
	cockpit.PinSession(sessions, "abc-1111", true) // pins do not count as recent
	config := filepath.Join(t.TempDir(), "settings.json")
	parse := func(args ...string) (options, error) {
		return parseOptions(append([]string{"-workspace", workspace, "-config", config}, args...), io.Discard)
	}
	for args, want := range map[string]string{"-continue": "abd-2222", "-session abc": "abc-1111", "-session brand-new": "brand-new"} {
		o, err := parse(strings.Fields(args)...)
		if err != nil || o.session != want {
			t.Errorf("%s: session %q, %v", args, o.session, err)
		}
	}
	for _, args := range []string{"-session ab", "-continue -session abc", "-resume -p hi", "-resume -continue"} {
		if _, err := parse(strings.Fields(args)...); err == nil {
			t.Errorf("%s was accepted", args)
		}
	}
	if o, err := parse("-provider", "anthropic", "-resume"); err != nil || !o.pick || o.SettingsFile != config {
		t.Errorf("-provider anthropic -resume: %+v, %v", o, err)
	}
}

func TestCtrlDTwiceQuitsDuringARun(t *testing.T) {
	m := testModel(t)
	m.state = running
	// The first press only asks; its command is a notice timer.
	if m.Update(key("ctrl+d")); m.quitArmed.IsZero() {
		t.Fatal("the first ctrl+d did not ask for confirmation")
	}
	if _, cmd := m.Update(key("ctrl+d")); !isQuit(cmd) {
		t.Fatal("the second ctrl+d did not quit")
	}
	m.state = idle
}

func TestStaleCheckIsIgnored(t *testing.T) {
	m := testModel(t)
	m.opt.SettingsFile = filepath.Join(t.TempDir(), "settings.json")
	m.command("/settings")
	m.Update(key("right"))
	m.Update(key("tab"))
	typeText(m, "http://127.0.0.1:1/v1")
	check := m.checkSettings()
	typeText(m, "2") // the form changes while the check runs
	m.Update(check())
	if m.form.level == "error" || strings.Contains(m.form.status, "reach") {
		t.Fatalf("a stale check reported: %q", m.form.status)
	}
}

func TestBranchResultAfterMovingOn(t *testing.T) {
	m := testModel(t)
	runPrompt(t, m, "alpha")
	cmd := m.fork("")
	m.newSession() // the view moves on before the branch lands
	m.Update(cmd())
	if m.note.level != "info" || !strings.Contains(m.note.text, "created") || !m.fresh {
		t.Fatalf("notice %+v, fresh %v", m.note, m.fresh)
	}
	m.Update(branchedMsg{gen: m.loadGen, err: os.ErrPermission})
	if m.note.level != "error" {
		t.Fatalf("a failed branch was not reported: %+v", m.note)
	}
}

func TestBrokenSettingsOpenForRepair(t *testing.T) {
	m := testModel(t)
	m.opt.SettingsFile = filepath.Join(t.TempDir(), "settings.json")
	os.WriteFile(m.opt.SettingsFile, []byte("{broken"), 0o600)
	m.command("/settings")
	if m.form == nil || m.form.level != "error" || !strings.Contains(m.form.status, "saving replaces them") {
		t.Fatalf("form %+v, notice %+v", m.form, m.note)
	}
	m.Update(key("right"))
	m.Update(key("right"))
	m.Update(key("enter"))
	if saved, err := cockpit.LoadSettings(m.opt.SettingsFile); err != nil || saved.API != cockpit.APIMessages || m.form != nil {
		t.Fatalf("saved %+v, %v", saved, err)
	}
}
