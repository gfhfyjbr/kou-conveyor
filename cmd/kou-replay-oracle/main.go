// Runs the real Go coordinator's restore path, stopping at the pure Resumed
// boundary before its scheduler can submit a tool or request a model response.
package main

import (
	"bufio"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"reflect"
	"slices"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/coordinator"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

type checkpoint struct {
	State jsontext.Value
	After uint64
	Items []jsontext.Value
}
type spec struct {
	StateProbe bool
	Items      []jsontext.Value
	Checkpoint *checkpoint
	Available  []string
	Model      llm.Model
	System     string
	Tools      []llm.Tool
	Pruning    struct {
		Budget                     int64
		Keep, Bytes, ThinkingTurns int
	}
	Masking contextbuilder.MaskOptions
}
type replayDone struct{}
type capturingBuilder struct {
	contextbuilder.Builder
	captured *contextbuilder.Result
}

func (current *capturingBuilder) Resumed() {
	current.Builder.Resumed()
	result, err := current.Builder.Build()
	if err != nil {
		panic(err)
	}
	current.captured = &result
	panic(replayDone{})
}
func (current *capturingBuilder) Checkpoint(live []string) (jsontext.Value, error) {
	return current.Builder.(contextbuilder.Checkpointer).Checkpoint(live)
}
func (current *capturingBuilder) RestoreCheckpoint(state jsontext.Value) error {
	return current.Builder.(contextbuilder.Checkpointer).RestoreCheckpoint(state)
}

type fakeStore struct {
	sessionstore.Store
	items []sessionstore.Item
}

func (store fakeStore) Items(_ context.Context, _ session.ID, after sessionstore.Sequence, limit int) (sessionstore.Page, error) {
	selected := make([]sessionstore.Item, 0)
	for _, item := range store.items {
		if item.Sequence > after {
			selected = append(selected, item)
		}
	}
	more := len(selected) > limit
	if more {
		selected = selected[:limit]
	}
	next := after
	if len(selected) > 0 {
		next = selected[len(selected)-1].Sequence
	}
	return sessionstore.Page{Items: selected, NextAfter: next, More: more}, nil
}

type fakeRegistry struct {
	tool.Registry
	names []string
}

func (registry fakeRegistry) Resolve(name string) (tool.Translator, bool) {
	if slices.Contains(registry.names, name) {
		return fakeTranslator{name: name}, true
	}
	return nil, false
}

type fakeTranslator struct{ name string }

func (fakeTranslator) Translate(tool.Context, llm.ToolCall) tool.CallStatus {
	panic("replay attempted a tool dispatch")
}
func operationText(values []operation.Operation) string {
	parts := make([]string, 0, len(values))
	for _, value := range values {
		parts = append(parts, string(value.ID)+":"+string(value.Status))
	}
	return strings.Join(parts, ",")
}
func (translator fakeTranslator) TranslateResult(id string, status tool.CallStatus, operations []operation.Operation) (llm.ToolResult, error) {
	if translator.name == "Reject" {
		return llm.ToolResult{}, errors.New("synthetic translate failure")
	}
	text := "translated " + id + ": " + operationText(operations)
	if status.Error != "" {
		text = "Error: " + status.Error
	}
	return llm.ToolResult{CallID: id, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: text}}}, nil
}
func (translator fakeTranslator) DescribeRunning(operations []operation.Operation) string {
	if translator.name == "Describe" {
		return "progress " + operationText(operations)
	}
	return ""
}

func decodeItems(raw []jsontext.Value) ([]sessionstore.Item, error) {
	result := make([]sessionstore.Item, 0, len(raw))
	for _, encoded := range raw {
		var envelope struct {
			Sequence   sessionstore.Sequence
			RecordedAt time.Time
			Kind       sessionstore.ItemKind
			Data       jsontext.Value
		}
		if err := json.Unmarshal(encoded, &envelope); err != nil {
			return nil, err
		}
		var data any
		switch envelope.Kind {
		case sessionstore.ItemFork:
			data = new(sessionstore.Fork)
		case sessionstore.ItemInput:
			data = new(inbox.Input)
		case sessionstore.ItemTurn:
			data = new(session.Turn)
		case sessionstore.ItemModelResponse:
			data = new(sessionstore.ModelResponse)
		case sessionstore.ItemToolCallStatus:
			data = new(sessionstore.ToolCallStatus)
		default:
			return nil, fmt.Errorf("unsupported fixture kind %q", envelope.Kind)
		}
		if err := json.Unmarshal(envelope.Data, data); err != nil {
			return nil, err
		}
		switch value := data.(type) {
		case *sessionstore.Fork:
			data = *value
		case *session.Turn:
			data = *value
		case *sessionstore.ModelResponse:
			data = *value
		case *sessionstore.ToolCallStatus:
			data = *value
		case *inbox.Input:
			data = *value
		}
		result = append(result, sessionstore.Item{Sequence: envelope.Sequence, RecordedAt: envelope.RecordedAt, Kind: envelope.Kind, Data: data})
	}
	return result, nil
}
func run(input spec) (output any) {
	items, err := decodeItems(input.Items)
	if err != nil {
		return map[string]any{"Error": err.Error()}
	}
	builder := contextbuilder.NewBuilder()
	builder.SetModel(input.Model)
	builder.SetSystemPrompt(input.System)
	builder.SetTools(input.Tools)
	builder.SetPruning(contextbuilder.PruneOptions{Budget: input.Pruning.Budget, Keep: input.Pruning.Keep, Bytes: input.Pruning.Bytes, ThinkingTurns: input.Pruning.ThinkingTurns})
	builder.SetMasking(input.Masking)
	captured := &capturingBuilder{Builder: builder}
	var restored sessionstore.ResumeState
	if input.Checkpoint != nil {
		tail, err := decodeItems(input.Checkpoint.Items)
		if err != nil {
			return map[string]any{"Error": err.Error()}
		}
		restored.Checkpoint = &sessionstore.ResumeCheckpoint{State: input.Checkpoint.State, After: sessionstore.Sequence(input.Checkpoint.After), Items: tail}
	}
	names := input.Available
	if names == nil {
		names = []string{"Read", "Write", "Edit", "apply_patch", "Bash", "Describe", "Reject"}
	}
	agent := coordinator.New(coordinator.Dependencies{SessionID: "synthetic", Sessions: fakeStore{items: items}, Restored: restored, ContextBuilder: captured, Tools: fakeRegistry{names: names}})
	defer func() {
		if recovered := recover(); recovered != nil {
			if _, ok := recovered.(replayDone); !ok {
				panic(recovered)
			}
			result := map[string]any{"Build": *captured.captured}
			if input.StateProbe {
				result["Probe"] = probe(agent)
			}
			output = result
		}
	}()
	err = agent.Run(context.Background())
	if err != nil {
		return map[string]any{"Error": err.Error()}
	}
	panic("coordinator ran beyond the replay boundary")
}

// This development-only probe reads scalar private state without unsafe or
// mutating the production package. Context output alone cannot detect a wrong
// pending-input count, breaker, verification history, or orphan operation.
func probe(agent coordinator.Coordinator) map[string]any {
	owner := reflect.ValueOf(agent).Elem()
	state := owner.FieldByName("state")
	result := map[string]any{}
	for _, name := range []string{"availableInputs", "deliveredInputs", "currentTurnInputs", "turnsSinceCompaction", "compactionFailures", "rapidRefills", "verifications", "heartbeats", "fileChanges"} {
		result[name] = state.FieldByName(name).Int()
	}
	for _, name := range []string{"compacted", "compactionAsked", "reminded", "workSinceVerification"} {
		result[name] = state.FieldByName(name).Bool()
	}
	for _, name := range []string{"currentTurnID", "currentTurnType"} {
		result[name] = state.FieldByName(name).String()
	}
	calls := map[string]string{}
	iter := state.FieldByName("toolCalls").MapRange()
	for iter.Next() {
		key := iter.Key()
		calls[key.FieldByName("turnID").String()+"/"+key.FieldByName("callID").String()] = iter.Value().FieldByName("toolCall").FieldByName("Name").String()
	}
	result["calls"] = calls
	operations := map[string]string{}
	iter = state.FieldByName("operations").MapRange()
	for iter.Next() {
		operations[iter.Key().String()] = iter.Value().FieldByName("Status").String()
	}
	result["operations"] = operations
	history := owner.FieldByName("history")
	result["historyWork"] = history.FieldByName("WorkSinceVerification").Bool()
	result["historyVerifications"] = history.FieldByName("Verifications").Int()
	var stop any
	if value := history.FieldByName("Stopped"); !value.IsNil() {
		stop = value.Elem().FieldByName("Reason").String()
	}
	result["historyStop"] = stop
	return result
}
func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 64<<10), 256<<20)
	for scanner.Scan() {
		var input spec
		var output any
		if err := json.Unmarshal(scanner.Bytes(), &input); err != nil {
			output = map[string]any{"Error": err.Error()}
		} else {
			output = run(input)
		}
		encoded, err := json.Marshal(output, json.Deterministic(true))
		if err != nil {
			panic(err)
		}
		fmt.Println(string(encoded))
	}
	if err := scanner.Err(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
