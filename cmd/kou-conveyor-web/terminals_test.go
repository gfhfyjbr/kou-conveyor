//go:build unix

package main

import (
	"encoding/json/v2"
	"net/http"
	"os"
	"strings"
	"testing"
	"time"

	"github.com/gorilla/websocket"
)

// quietShell has terminals start a shell with no startup files to wait for.
func quietShell(t *testing.T) {
	t.Helper()
	home := t.TempDir()
	t.Setenv("SHELL", "/bin/sh")
	t.Setenv("HOME", home)
	t.Setenv("ENV", "")
	t.Setenv("PS1", "$ ")
}

// dialTerminal attaches to a terminal as a page of the cockpit would.
func (h *harness) dialTerminal(id string, origin string) (*websocket.Conn, *http.Response, error) {
	h.Helper()
	url := "ws" + strings.TrimPrefix(h.http.URL, "http") + "/api/terminals/" + id + "/socket"
	header := http.Header{}
	if origin != "" {
		header.Set("Origin", origin)
	}
	return websocket.DefaultDialer.Dial(url, header)
}

// readUntil reads what the socket brings until want is in its output, and
// returns the output and the messages.
func readUntil(t *testing.T, conn *websocket.Conn, want string) (string, []map[string]any) {
	t.Helper()
	var out strings.Builder
	var messages []map[string]any
	conn.SetReadDeadline(time.Now().Add(15 * time.Second))
	for {
		kind, data, err := conn.ReadMessage()
		if err != nil {
			t.Fatalf("no %q in %q (%v): %v", want, out.String(), messages, err)
		}
		if kind == websocket.BinaryMessage {
			out.Write(data)
		} else {
			var m map[string]any
			if err := json.Unmarshal(data, &m); err != nil {
				t.Fatal(err)
			}
			messages = append(messages, m)
			if m["type"] == want {
				return out.String(), messages
			}
		}
		if want != "" && strings.Contains(out.String(), want) {
			return out.String(), messages
		}
	}
}

func TestTerminalRunsAShellOverAWebSocket(t *testing.T) {
	quietShell(t)
	h := newHarness(t)
	res, started := h.do("POST", "/api/terminals", `{"cols": 90, "rows": 20}`)
	if res.StatusCode != http.StatusCreated || started["id"] == "" || started["cols"] != 90.0 {
		t.Fatalf("start: %d %v", res.StatusCode, started)
	}
	id := started["id"].(string)
	conn, _, err := h.dialTerminal(id, h.http.URL)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	_, messages := readUntil(t, conn, "live")
	if messages[0]["type"] != "hello" || messages[0]["terminal"].(map[string]any)["id"] != id {
		t.Fatalf("hello = %v", messages)
	}
	conn.WriteMessage(websocket.BinaryMessage, []byte("printf '\\033]2;my%s\\007' title; echo out-$((20+2))\n"))
	_, messages = readUntil(t, conn, "out-22")
	titled := false
	for _, m := range messages {
		titled = titled || m["type"] == "meta" && m["title"] == "mytitle"
	}
	if !titled {
		t.Fatalf("no title in %v", messages)
	}
	conn.WriteMessage(websocket.TextMessage, []byte(`{"type":"resize","cols":120,"rows":33}`))
	time.Sleep(400 * time.Millisecond)
	conn.WriteMessage(websocket.BinaryMessage, []byte("stty size\n"))
	readUntil(t, conn, "33 120")

	// Listed with its workspace, and reattached with what it printed.
	_, listed := h.do("GET", "/api/terminals", "")
	if list := listed["terminals"].([]any); len(list) != 1 {
		t.Fatalf("list = %v", listed)
	}
	conn.Close()
	again, _, err := h.dialTerminal(id, h.http.URL)
	if err != nil {
		t.Fatal(err)
	}
	defer again.Close()
	replay, messages := readUntil(t, again, "live")
	if !strings.Contains(replay, "out-22") || messages[0]["replay"].(float64) == 0 {
		t.Fatalf("replay = %q, %v", replay, messages)
	}

	// Closing the tab ends the shell.
	if res, _ := h.do("DELETE", "/api/terminals/"+id, ""); res.StatusCode != http.StatusOK {
		t.Fatalf("kill: %d", res.StatusCode)
	}
	_, messages = readUntil(t, again, "exit")
	if messages[len(messages)-1]["type"] != "exit" {
		t.Fatalf("messages = %v", messages)
	}
}

func TestTerminalRefusesOtherOrigins(t *testing.T) {
	quietShell(t)
	h := newHarness(t)
	_, started := h.do("POST", "/api/terminals", `{}`)
	id := started["id"].(string)
	defer h.do("DELETE", "/api/terminals/"+id, "")
	if _, res, err := h.dialTerminal(id, "http://evil.example"); err == nil || res == nil || res.StatusCode != http.StatusForbidden {
		t.Fatalf("a page of another site attached: %v, %v", res, err)
	}
	if _, res, err := h.dialTerminal("nope", h.http.URL); err == nil || res == nil || res.StatusCode != http.StatusNotFound {
		t.Fatalf("an unknown terminal: %v, %v", res, err)
	}
}

func TestTerminalStartsWhereAnotherIs(t *testing.T) {
	quietShell(t)
	h := newHarness(t)
	sub := h.server.workspaces.startup().Path + "/sub"
	if err := os.Mkdir(sub, 0o700); err != nil {
		t.Fatal(err)
	}
	_, first := h.do("POST", "/api/terminals", `{"cwd": "sub"}`)
	id := first["id"].(string)
	defer h.do("DELETE", "/api/terminals/"+id, "")
	_, second := h.do("POST", "/api/terminals", `{"from": "`+id+`"}`)
	defer h.do("DELETE", "/api/terminals/"+second["id"].(string), "")
	conn, _, err := h.dialTerminal(second["id"].(string), h.http.URL)
	if err != nil {
		t.Fatal(err)
	}
	defer conn.Close()
	readUntil(t, conn, "live")
	conn.WriteMessage(websocket.BinaryMessage, []byte("echo \"at=$(basename \"$PWD\")=\"\n"))
	readUntil(t, conn, "at=sub=")
	if res, body := h.do("POST", "/api/terminals", `{"cwd": "missing"}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("a missing folder: %d %v", res.StatusCode, body)
	}
}

func TestTerminalSettingsTurnTheThemeOff(t *testing.T) {
	h := newHarness(t)
	if _, got := h.do("GET", "/api/terminal/settings", ""); got["theme"] != "kou" {
		t.Fatalf("settings = %v", got)
	}
	if !h.server.terminalTheme() {
		t.Fatal("the theme is off by default")
	}
	if res, got := h.do("PUT", "/api/terminal/settings", `{"theme": "shell"}`); res.StatusCode != http.StatusOK || got["theme"] != "shell" {
		t.Fatalf("save = %d %v", res.StatusCode, got)
	}
	if h.server.terminalTheme() {
		t.Fatal("the theme stays on")
	}
	if res, _ := h.do("PUT", "/api/terminal/settings", `{"theme": "fancy"}`); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("an unknown theme: %d", res.StatusCode)
	}
}

func TestPolicyLetsTheTerminalRun(t *testing.T) {
	h := newHarness(t)
	res, _ := h.do("GET", "/api/config", "")
	policy := res.Header.Get("Content-Security-Policy")
	host := strings.TrimPrefix(h.http.URL, "http://")
	for _, want := range []string{"'wasm-unsafe-eval'", "connect-src 'self' ws://" + host + " wss://" + host, "style-src 'self' 'sha256-"} {
		if !strings.Contains(policy, want) {
			t.Fatalf("policy %q lacks %q", policy, want)
		}
	}
	if got := sockets("evil.com; script-src *"); got != "" {
		t.Fatalf("sockets = %q", got)
	}
}

func TestTerminalClosedLaterCanBeKept(t *testing.T) {
	quietShell(t)
	h := newHarness(t)
	_, started := h.do("POST", "/api/terminals", `{}`)
	id := started["id"].(string)
	defer h.do("DELETE", "/api/terminals/"+id, "")
	res, info := h.do("DELETE", "/api/terminals/"+id+"?after=15", "")
	if res.StatusCode != http.StatusOK || info["closing_at"] == nil {
		t.Fatalf("close later: %d %v", res.StatusCode, info)
	}
	res, info = h.do("POST", "/api/terminals/"+id+"/keep", "")
	if res.StatusCode != http.StatusOK || info["closing_at"] != nil || info["exited"] != nil {
		t.Fatalf("keep: %d %v", res.StatusCode, info)
	}
	if res, _ := h.do("POST", "/api/terminals/nope/keep", ""); res.StatusCode != http.StatusNotFound {
		t.Fatalf("keep unknown: %d", res.StatusCode)
	}
}
