// Package cockpittest impersonates kou-conveyor-runner, so front-end tests can
// drive real runner processes without a model provider.
package cockpittest

import (
	"bytes"
	"encoding/base64"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"flag"
	"fmt"
	"image"
	"image/color"
	"image/png"
	"io"
	"os"
	"os/signal"
	"path/filepath"
	"strings"
	"syscall"
	"testing"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

const enable = "COCKPITTEST_FAKE_RUNNER"

// NoSteer, set to 1, makes the fake runner one built before runners took
// messages while they ran.
const NoSteer = "COCKPITTEST_NO_STEER"

// Main turns the test binary into a fake runner when a test launched it
// through Runner. Call it first in TestMain.
func Main() {
	if os.Getenv(enable) != "1" {
		return
	}
	os.Exit(run(os.Args[1:], os.Stdin, os.Stdout, os.Stderr))
}

// Runner returns an executable that behaves like kou-conveyor-runner and
// persists sessions in the runner's file format. The prompt picks the script:
//
//   - "wait": start a turn and block until interrupted;
//   - "fail": report an error event and exit 1;
//   - "crash": write to stderr and exit 2 without an event;
//   - "tool": run one Bash call before answering;
//   - "touch": like "tool", and the call writes touched.txt in the workspace:
//     the prompt, and on later runs what the file had, below it;
//   - "print-env": answer with the connection variables the runner got;
//   - "print-thinking": answer with the thinking level it was asked for;
//   - "steer": run one Bash call and wait for a message on stdin (-steer)
//     while it runs; the message is recorded after the call's result, as the
//     runner records one delivered after tools, and answered with
//     "steered: <message>";
//   - "gate": block until a file named gate exists in the workspace, or
//     until interrupted;
//   - "look": run one ViewImage call, which reads Picture (480×320 PNG);
//   - anything else: answer "echo: <prompt>".
//
// With "slow" in the prompt, items are paced so streaming can be watched.
//
// A compaction request writes a compaction turn whose summary names the
// instructions; with "wait" in them it blocks until interrupted, and with
// "nothing" the summary is empty.
func Runner(t testing.TB) string {
	t.Helper()
	t.Setenv(enable, "1")
	exe, err := os.Executable()
	if err != nil {
		t.Fatal(err)
	}
	return exe
}

func run(args []string, stdin io.Reader, stdout, stderr io.Writer) int {
	flags := flag.NewFlagSet("fake-runner", flag.ContinueOnError)
	flags.SetOutput(stderr)
	sessions := flags.String("session-directory", "", "")
	workspace := flags.String("workspace", "", "")
	flags.String("log-directory", "", "")
	flags.Duration("tool-heartbeat-interval", 0, "")
	providers := flags.Bool("providers", false, "")
	steer := new(bool)
	if os.Getenv(NoSteer) != "1" {
		steer = flags.Bool("steer", false, "keep reading stdin for messages")
	}
	if err := flags.Parse(args); err != nil {
		return 2
	}
	if *providers {
		fmt.Fprintln(stdout, "ollama\nopenai\nanthropic\nopenai-codex\nopenrouter\nfireworks")
		return 0
	}
	var request struct {
		Messages []struct {
			Content   string         `json:"content"`
			MessageID string         `json:"message_id"`
			Images    []requestImage `json:"images"`
		} `json:"messages"`
		SessionID    string `json:"session_id"`
		Model        string `json:"model"`
		Thinking     string `json:"thinking_level"`
		Compact      bool   `json:"compact"`
		Instructions string `json:"compact_instructions"`
	}
	// With -steer the request is the first value on stdin; messages follow.
	decoder := jsontext.NewDecoder(stdin)
	if err := json.UnmarshalDecode(decoder, &request); err != nil || len(request.Messages) != 1 && !request.Compact {
		fmt.Fprintln(stderr, "fake-runner: bad request:", err)
		return 2
	}
	// Turns record the model their requests go to, as the runner's do: the
	// request's, else the environment's.
	model := request.Model
	if model == "" {
		model = os.Getenv("KOU_CONVEYOR_LLM_MODEL")
	}
	if request.Compact {
		return compact(*sessions, request.SessionID, request.Instructions, model, stdout, stderr)
	}
	prompt := request.Messages[0].Content
	switch {
	case strings.Contains(prompt, "fail"):
		fmt.Fprintln(stdout, `{"type":"error","message":"fake failure"}`)
		fmt.Fprintln(stderr, "fake-runner: fake failure")
		return 1
	case strings.Contains(prompt, "crash"):
		fmt.Fprintln(stderr, "panic: fake crash")
		return 2
	}

	s, err := openStore(*sessions, request.SessionID, stdout)
	if err != nil {
		fmt.Fprintln(stderr, "fake-runner:", err)
		return 1
	}
	s.model = model
	if strings.Contains(prompt, "slow") {
		s.pace = 700 * time.Millisecond
	}
	images := request.Messages[0].Images
	payload := promptPayload(prompt, images)
	s.emit(sessionstore.ItemInput, inbox.Input{
		ID: inbox.ID(request.Messages[0].MessageID), Kind: inbox.InputExternal, Payload: payload,
	})
	turn := s.turn()
	if strings.Contains(prompt, "wait") {
		interrupted := make(chan os.Signal, 1)
		signal.Notify(interrupted, os.Interrupt, syscall.SIGTERM)
		<-interrupted
		return 130
	}
	if strings.Contains(prompt, "gate") {
		interrupted := make(chan os.Signal, 1)
		signal.Notify(interrupted, os.Interrupt, syscall.SIGTERM)
		for {
			if _, err := os.Stat(filepath.Join(*workspace, "gate")); err == nil {
				break
			}
			select {
			case <-interrupted:
				return 130
			case <-time.After(20 * time.Millisecond):
			}
		}
	}
	steered := ""
	if strings.Contains(prompt, "steer") && *steer {
		// A long command, and a message for the agent while it runs.
		s.emit(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: turn, Response: llm.Response{
			Stop: llm.StopComplete,
			Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{
				CallID: "call-steer", Name: "Bash", Arguments: `{"command":"make test"}`,
			}}},
			Usage: llm.Usage{InputTokens: 100, OutputTokens: 10},
		}})
		shell := func(status operation.Status, result *operation.ShellResult) []operation.Operation {
			state, _ := json.Marshal(operation.ShellState{Input: operation.ShellInput{Command: "make test"}, Result: result})
			return []operation.Operation{{
				ID: "operation-steer", Type: operation.TypeShell, Version: operation.VersionShell,
				Status: status, State: state,
			}}
		}
		waiting := tool.CallStatus{WaitingFor: []operation.ID{"operation-steer"}}
		s.emit(sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: turn, CallID: "call-steer", Status: waiting, Operations: shell(operation.StatusAwaiting, nil),
		})
		message, received := receive(decoder)
		s.emit(sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: turn, CallID: "call-steer", Status: waiting,
			Operations: shell(operation.StatusCompleted, &operation.ShellResult{Out: "ok\n", OutSize: 3}),
		})
		if received {
			payload := promptPayload(message.Content, message.Images)
			s.emit(sessionstore.ItemInput, inbox.Input{
				ID: inbox.ID(message.MessageID), Kind: inbox.InputExternal, Payload: payload, Delivery: inbox.DeliverAfterTools,
			})
			turn = s.turn()
			steered = message.Content
		}
	}
	if strings.Contains(prompt, "touch") {
		path := filepath.Join(*workspace, "touched.txt")
		old, _ := os.ReadFile(path)
		os.WriteFile(path, append([]byte(prompt+"\n"), old...), 0o600)
	}
	if strings.Contains(prompt, "look") {
		turn = s.look(turn)
	}
	if strings.Contains(prompt, "tool") || strings.Contains(prompt, "touch") {
		s.emit(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: turn, Response: llm.Response{
			Stop: llm.StopComplete,
			Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{
				CallID: "call-1", Name: "Bash", Arguments: `{"command":"echo hi"}`,
			}}},
			Usage: llm.Usage{InputTokens: 100, OutputTokens: 10},
		}})
		shell := func(status operation.Status, result *operation.ShellResult) []operation.Operation {
			state, _ := json.Marshal(operation.ShellState{Input: operation.ShellInput{Command: "echo hi"}, Result: result})
			return []operation.Operation{{
				ID: "operation-1", Type: operation.TypeShell, Version: operation.VersionShell,
				Status: status, State: state,
			}}
		}
		waiting := tool.CallStatus{WaitingFor: []operation.ID{"operation-1"}}
		s.emit(sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: turn, CallID: "call-1", Status: waiting, Operations: shell(operation.StatusReady, nil),
		})
		s.emit(sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
			TurnID: turn, CallID: "call-1", Status: waiting,
			Operations: shell(operation.StatusCompleted, &operation.ShellResult{Out: "hi\n", OutSize: 3}),
		})
		turn = s.turn()
	}
	answer := "echo: " + prompt
	if len(images) != 0 {
		answer += fmt.Sprintf(" · %d images", len(images))
	}
	if strings.Contains(prompt, "print-env") {
		var values []string
		for _, name := range []string{"PROVIDER", "BASE_URL", "API_KEY", "MODEL"} {
			value, set := os.LookupEnv("KOU_CONVEYOR_LLM_" + name)
			if !set {
				value = "<unset>"
			}
			values = append(values, strings.ToLower(name)+"="+value)
		}
		answer = strings.Join(values, " ")
	}
	if strings.Contains(prompt, "print-thinking") {
		answer = "thinking=" + request.Thinking
	}
	if steered != "" {
		answer = "steered: " + steered
	}
	s.emit(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: turn, Response: llm.Response{
		Stop: llm.StopComplete,
		Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{
			Role: llm.RoleAssistant, Text: answer, Phase: "final_answer",
		}}},
		Usage: llm.Usage{InputTokens: 120, CachedInputTokens: 100, OutputTokens: 12},
	}})
	return 0
}

// Picture is what the "look" script's ViewImage call reads: a 480×320 PNG.
func Picture() []byte {
	img := image.NewNRGBA(image.Rect(0, 0, 480, 320))
	for y := range 320 {
		for x := range 480 {
			img.SetNRGBA(x, y, color.NRGBA{uint8(x / 2), uint8(y * 4 / 5), 160, 255})
		}
	}
	var out bytes.Buffer
	if err := png.Encode(&out, img); err != nil {
		panic(err)
	}
	return out.Bytes()
}

// look runs one ViewImage call, as the runner records one: its call, the
// status that waits for its operation, and the status with the picture it
// read. Every call has an ID of its own.
func (s *store) look(turn session.TurnID) session.TurnID {
	call, id := "call-look-"+uuid.New().String()[:8], operation.ID(uuid.New().String())
	s.emit(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: turn, Response: llm.Response{
		Stop: llm.StopComplete,
		Output: []llm.Item{{Type: llm.ItemToolCall, Data: llm.ToolCall{
			CallID: call, Name: "ViewImage", Arguments: `{"path":"shots/screen.png"}`,
		}}},
		Usage: llm.Usage{InputTokens: 100, OutputTokens: 10},
	}})
	view := func(status operation.Status, result *operation.ViewImageResult) []operation.Operation {
		state, _ := json.Marshal(operation.ViewImageState{
			Path: "/workspace/shots/screen.png", Config: operation.ViewImageConfig{MaxSize: 4_999_000, MaxWidth: 2000, MaxHeight: 2000}, Result: result,
		})
		return []operation.Operation{{ID: id, Type: operation.TypeViewImage, Version: operation.VersionViewImage, Status: status, State: state}}
	}
	waiting := tool.CallStatus{WaitingFor: []operation.ID{id}}
	s.emit(sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
		TurnID: turn, CallID: call, Status: waiting, Operations: view(operation.StatusReady, nil),
	})
	s.emit(sessionstore.ItemToolCallStatus, sessionstore.ToolCallStatus{
		TurnID: turn, CallID: call, Status: waiting, Operations: view(operation.StatusCompleted, &operation.ViewImageResult{
			Content: base64.StdEncoding.EncodeToString(Picture()), OriginalWidth: 480, OriginalHeight: 320,
			OriginalMIMEType: "image/png", EncodedMIMEType: "image/png", ScaleRatio: 1,
		}),
	})
	return s.turn()
}

func compact(dir, id, instructions, model string, stdout, stderr io.Writer) int {
	s, err := openStore(dir, id, stdout)
	if err != nil {
		fmt.Fprintln(stderr, "fake-runner:", err)
		return 1
	}
	s.model = model
	control, _ := json.Marshal(inbox.ControlMessage{Mode: inbox.Compact, Reason: instructions})
	s.emit(sessionstore.ItemInput, inbox.Input{ID: inbox.ID(uuid.New().String()), Kind: inbox.InputControl, Payload: control})
	turn := s.nextTurn(session.TurnCompaction)
	if strings.Contains(instructions, "wait") {
		interrupted := make(chan os.Signal, 1)
		signal.Notify(interrupted, os.Interrupt, syscall.SIGTERM)
		<-interrupted
		return 130
	}
	// The model thinks the conversation through before the summary, which
	// is all the runner keeps.
	answer := "<analysis>\nThe user asked questions.\n</analysis>"
	if !strings.Contains(instructions, "nothing") {
		summary := "Summary of the session"
		if instructions != "" {
			summary += ", focused on " + instructions
		}
		answer += "\n\n<summary>\n" + summary + "\n</summary>"
	}
	s.emit(sessionstore.ItemModelResponse, sessionstore.ModelResponse{TurnID: turn, Response: llm.Response{
		Stop:   llm.StopComplete,
		Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: answer}}},
		Usage:  llm.Usage{InputTokens: 152_000, OutputTokens: 900},
	}})
	return 0
}

// store appends items to a session file in the runner's format and mirrors
// them to stdout the way the runner's observer does.
type store struct {
	file     *os.File
	stdout   io.Writer
	sequence sessionstore.Sequence
	lastTurn session.TurnID
	pace     time.Duration
	model    string // the model turns record
}

func openStore(dir, id string, stdout io.Writer) (*store, error) {
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, err
	}
	path := filepath.Join(dir, id+".session.jsonl")
	existing, err := os.ReadFile(path)
	if err != nil && !errors.Is(err, os.ErrNotExist) {
		return nil, err
	}
	file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return nil, err
	}
	s := &store{file: file, stdout: stdout}
	if len(existing) == 0 {
		header, _ := json.Marshal(map[string]any{"type": "session", "data": map[string]any{
			"Version": 2, "Session": session.Session{ID: session.ID(id), CreatedAt: time.Now().UTC()},
		}})
		file.Write(append(header, '\n'))
	}
	// Continue the item sequence and the turn chain, which the runner's
	// store validates when it reads the file.
	for line := range bytes.Lines(existing) {
		var record struct {
			Type string `json:"type"`
			Data struct {
				Item struct {
					Kind string
					Data struct{ ID string }
				}
			} `json:"data"`
		}
		if json.Unmarshal(line, &record) != nil || record.Type != "item" {
			continue
		}
		s.sequence++
		if record.Data.Item.Kind == string(sessionstore.ItemTurn) {
			s.lastTurn = session.TurnID(record.Data.Item.Data.ID)
		}
	}
	return s, nil
}

func (s *store) turn() session.TurnID { return s.nextTurn(session.TurnRegular) }

func (s *store) nextTurn(kind session.TurnType) session.TurnID {
	id := session.TurnID(uuid.New().String())
	s.emit(sessionstore.ItemTurn, session.Turn{ID: id, PreviousTurnID: s.lastTurn, Type: kind, Model: s.model})
	s.lastTurn = id
	return id
}

func (s *store) emit(kind sessionstore.ItemKind, data any) {
	time.Sleep(s.pace)
	s.sequence++
	item := sessionstore.Item{Sequence: s.sequence, RecordedAt: time.Now().UTC(), Kind: kind, Data: data}
	live, err := json.Marshal(item)
	if err != nil {
		panic(err)
	}
	var operations []operation.Operation
	if status, ok := data.(sessionstore.ToolCallStatus); ok {
		operations, status.Operations = status.Operations, nil
		item.Data = status
	}
	persisted, err := json.Marshal(struct {
		Type string `json:"type"`
		Data any    `json:"data"`
	}{"item", struct {
		Item       sessionstore.Item
		Operations []operation.Operation `json:",omitempty"`
	}{item, operations}})
	if err != nil {
		panic(err)
	}
	s.file.Write(append(persisted, '\n'))
	s.stdout.Write(append(live, '\n'))
}

type steeringMessage struct {
	Content   string         `json:"content"`
	MessageID string         `json:"message_id"`
	Images    []requestImage `json:"images"`
}

// requestImage is an image of a prompt, as the runner takes it.
type requestImage struct {
	Label     string `json:"label"`
	MediaType string `json:"media_type"`
	Data      []byte `json:"data"`
}

// promptPayload records a prompt and its images as the runner does.
func promptPayload(text string, images []requestImage) jsontext.Value {
	var recorded []llm.Image
	for _, image := range images {
		recorded = append(recorded, llm.Image{
			Label: image.Label, URL: "data:" + image.MediaType + ";base64," + base64.StdEncoding.EncodeToString(image.Data),
		})
	}
	payload, err := contextbuilder.ExternalPayload(text, recorded)
	if err != nil {
		panic(err)
	}
	return payload
}

// receive waits a while for a message on stdin.
func receive(decoder *jsontext.Decoder) (steeringMessage, bool) {
	received := make(chan steeringMessage, 1)
	go func() {
		var message steeringMessage
		if json.UnmarshalDecode(decoder, &message) == nil {
			received <- message
		}
	}()
	select {
	case message := <-received:
		return message, true
	case <-time.After(15 * time.Second):
		return steeringMessage{}, false
	}
}
