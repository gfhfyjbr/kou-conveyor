package canvascli

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"io"
	"os"
	"strings"
	"time"
)

// Hooks tell the canvas what a harness does: Claude Code runs kou-canvas
// hook claude at the events of its settings, with the event as JSON on
// stdin; Codex runs kou-canvas hook codex as its notify program, with the
// event as JSON in the last argument. A hook says nothing, fails nothing
// and waits for little: what goes wrong is the canvas's loss, never the
// harness's.

// hookTimeout bounds what a hook waits for the server.
const hookTimeout = 3 * time.Second

func (a *app) hook(ctx context.Context, args []string) {
	if len(args) == 0 {
		return
	}
	c, err := a.canvasClient()
	if err != nil {
		return // not on a canvas: nothing to tell
	}
	ctx, cancel := context.WithTimeout(ctx, hookTimeout)
	defer cancel()
	var report map[string]any
	switch args[0] {
	case "claude":
		input, _ := io.ReadAll(io.LimitReader(a.in, 4<<20))
		report = claudeReport(input)
	case "codex":
		if len(args) > 1 {
			report = codexReport([]byte(args[len(args)-1]))
		}
	}
	if report == nil {
		return
	}
	_, _ = c.call(ctx, "POST", "/api/canvas/emit", nil, report, nil)
}

// claudeEvent is what Claude Code gives its hooks.
type claudeEvent struct {
	Session          string `json:"session_id"`
	Transcript       string `json:"transcript_path"`
	Event            string `json:"hook_event_name"`
	Message          string `json:"message"`
	NotificationType string `json:"notification_type"`
	// LastAssistantMessage is the answer, where Claude Code gives it.
	LastAssistantMessage string `json:"last_assistant_message"`
}

// claudeReport is what a Claude Code event tells the canvas, nil for
// nothing.
func claudeReport(input []byte) map[string]any {
	var e claudeEvent
	if json.Unmarshal(input, &e) != nil {
		return nil
	}
	switch e.Event {
	case "SessionStart":
		report := map[string]any{"status": "idle"}
		if e.Session != "" {
			report["agent_session"] = e.Session
		}
		return report
	case "UserPromptSubmit":
		return map[string]any{"status": "busy"}
	case "Stop":
		// The answer the turn ended with — or, without text, that it
		// answered nothing.
		report := map[string]any{"status": "idle", "answer": true}
		answer := strings.TrimSpace(e.LastAssistantMessage)
		if answer == "" && e.Transcript != "" {
			answer = claudeAnswer(e.Transcript)
		}
		if answer != "" {
			report["text"] = answer
		}
		if e.Session != "" {
			report["agent_session"] = e.Session
		}
		return report
	case "Notification":
		// Claude Code also says it waits for input once it has been idle a
		// while: it is idle, not waiting for the user.
		if e.NotificationType == "idle_prompt" || strings.Contains(strings.ToLower(e.Message), "waiting for your input") {
			return nil
		}
		return map[string]any{"status": "waiting", "detail": e.Message}
	}
	return nil
}

// claudeAnswer reads the last answer of a Claude Code transcript: the
// text of its last assistant message. The transcript may lag behind the
// Stop event a little, so a transcript whose last message is not the
// assistant's is read again, for a moment.
func claudeAnswer(path string) string {
	for attempt := 0; attempt < 6; attempt++ {
		if answer, done := readClaudeAnswer(path); done || attempt == 5 {
			return answer
		}
		time.Sleep(150 * time.Millisecond)
	}
	return ""
}

// readClaudeAnswer reads the transcript's last answer; done is false when
// the transcript does not end with one yet.
func readClaudeAnswer(path string) (string, bool) {
	file, err := os.Open(path)
	if err != nil {
		return "", true
	}
	defer file.Close()
	// The answer is at the end: the last few MiB hold it.
	const window = 4 << 20
	if info, err := file.Stat(); err == nil && info.Size() > window {
		if _, err := file.Seek(info.Size()-window, io.SeekStart); err != nil {
			return "", true
		}
	}
	type block struct {
		Type string `json:"type"`
		Text string `json:"text"`
	}
	type line struct {
		Type    string `json:"type"`
		Message struct {
			ID      string         `json:"id"`
			Role    string         `json:"role"`
			Content jsontext.Value `json:"content"`
		} `json:"message"`
	}
	var lines []line
	scanner := bufio.NewScanner(file)
	scanner.Buffer(nil, 16<<20)
	for scanner.Scan() {
		data := bytes.TrimSpace(scanner.Bytes())
		if len(data) == 0 || data[0] != '{' {
			continue
		}
		var l line
		if json.Unmarshal(data, &l) == nil && (l.Type == "assistant" || l.Type == "user") {
			lines = append(lines, l)
		}
	}
	textOf := func(l line) string {
		var blocks []block
		if json.Unmarshal(l.Message.Content, &blocks) != nil {
			var text string
			if json.Unmarshal(l.Message.Content, &text) == nil {
				return text
			}
			return ""
		}
		var parts []string
		for _, b := range blocks {
			if b.Type == "text" && strings.TrimSpace(b.Text) != "" {
				parts = append(parts, b.Text)
			}
		}
		return strings.Join(parts, "\n\n")
	}
	// The last assistant message with text; Claude Code writes the blocks
	// of a message as lines of their own, under the message's ID.
	for i := len(lines) - 1; i >= 0; i-- {
		l := lines[i]
		if l.Type == "user" {
			// A prompt, or a tool's result, after the last answer: the
			// answer is not written yet.
			return "", false
		}
		text := textOf(l)
		if text == "" {
			continue
		}
		parts := []string{text}
		for j := i - 1; j >= 0 && lines[j].Type == "assistant" && l.Message.ID != "" && lines[j].Message.ID == l.Message.ID; j-- {
			if earlier := textOf(lines[j]); earlier != "" {
				parts = append([]string{earlier}, parts...)
			}
		}
		return strings.TrimSpace(strings.Join(parts, "\n\n")), true
	}
	return "", false
}

// codexReport is what a Codex notification tells the canvas, nil for
// nothing.
func codexReport(input []byte) map[string]any {
	var e struct {
		Type   string `json:"type"`
		Thread string `json:"thread-id"`
		Answer string `json:"last-assistant-message"`
	}
	if json.Unmarshal(input, &e) != nil || e.Type != "agent-turn-complete" {
		return nil
	}
	report := map[string]any{"status": "idle", "answer": true}
	if answer := strings.TrimSpace(e.Answer); answer != "" {
		report["text"] = answer
	}
	if e.Thread != "" {
		report["agent_session"] = e.Thread
	}
	return report
}
