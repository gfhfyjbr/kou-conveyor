// The context migration oracle runs only pure builder operations on synthetic input.
package main

import (
	"bufio"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"os"
	"slices"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

type pruning struct {
	Budget        int64
	Keep, Bytes   int
	First         []string
	ThinkingTurns int
}
type spec struct {
	Skills                                  []tool.Skill
	Model                                   llm.Model
	System                                  string
	Tools                                   []llm.Tool
	Pruning                                 pruning
	Masking                                 contextbuilder.MaskOptions
	NotesPath, Notes, Summaries, Transcript string
	NotesError                              bool
	Steps                                   []step
}
type step struct {
	Action     string
	Input      inbox.Input
	Control    inbox.ControlMessage
	Response   llm.Response
	Reasoning  llm.Reasoning
	CallID     string
	Output     []llm.ToolResultOutput
	Running    bool
	Focus      string
	Budget     int64
	Kept       *contextbuilder.Compacted
	Live       []string
	Encoded    jsontext.Value
	Notes      string
	NotesError bool
	Model      llm.Model
	System     string
	Tools      []llm.Tool
	Skills     []tool.Skill
	Pruning    pruning
	Masking    contextbuilder.MaskOptions
}

func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 64<<10), 256<<20)
	for scanner.Scan() {
		var input spec
		if err := json.Unmarshal(scanner.Bytes(), &input); err != nil {
			emit(map[string]any{"Error": err.Error()})
			continue
		}
		emit(run(input))
	}
	if err := scanner.Err(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func emit(value any) {
	encoded, err := json.Marshal(value, json.Deterministic(true))
	if err != nil {
		panic(err)
	}
	fmt.Println(string(encoded))
}
func setPruning(current contextbuilder.Builder, options pruning) {
	var first func(string) bool
	if options.First != nil {
		first = func(name string) bool { return slices.Contains(options.First, name) }
	}
	current.SetPruning(contextbuilder.PruneOptions{Budget: options.Budget, Keep: options.Keep, Bytes: options.Bytes, ThinkingTurns: options.ThinkingTurns, First: first})
}
func run(input spec) []any {
	notes, notesError := input.Notes, input.NotesError
	newBuilder := func() contextbuilder.Builder {
		current := contextbuilder.NewBuilder(input.Skills...)
		current.SetModel(input.Model)
		current.SetSystemPrompt(input.System)
		current.SetTools(input.Tools)
		setPruning(current, input.Pruning)
		current.SetMasking(input.Masking)
		current.SetTranscript(input.Transcript)
		current.SetMemory(contextbuilder.Memory{Notes: input.NotesPath, Summaries: input.Summaries, ReadNotes: func() (string, error) {
			if notesError {
				return "", errors.New("synthetic notes failure")
			}
			return notes, nil
		}})
		return current
	}
	current := newBuilder()
	var saved jsontext.Value
	output := make([]any, 0)
	for _, step := range input.Steps {
		var err error
		switch step.Action {
		case "input":
			err = current.AddExternalInput(step.Input)
		case "control":
			if step.Control.Mode == inbox.UpdateSettings {
				// Reuse inbox's actual typed control decoding, not an oracle-only settings DTO.
				encoded, encodeErr := json.Marshal(step.Control)
				if encodeErr != nil {
					err = encodeErr
					break
				}
				decoded, decodeErr := (inbox.Input{Kind: inbox.InputControl, Payload: encoded}).DecodeControlMessage()
				if decodeErr != nil {
					err = decodeErr
					break
				}
				current.AddControlMessage(decoded)
			} else {
				current.AddControlMessage(step.Control)
			}
		case "response":
			current.AddModelResponse(step.Response)
		case "reasoning":
			current.AddReasoning(step.Reasoning)
		case "result":
			current.AddToolResult(step.CallID, step.Output, step.Running)
		case "refresh":
			current.(interface {
				RefreshToolResult(string, []llm.ToolResultOutput)
			}).RefreshToolResult(step.CallID, step.Output)
		case "commit":
			current.Commit()
		case "resumed":
			current.Resumed()
		case "model":
			current.SetModel(step.Model)
		case "system":
			current.SetSystemPrompt(step.System)
		case "tools":
			current.SetTools(step.Tools)
		case "skills":
			current.SetSkills(step.Skills)
		case "pruning":
			setPruning(current, step.Pruning)
		case "masking":
			current.SetMasking(step.Masking)
		case "build":
			var value contextbuilder.Result
			value, err = current.Build()
			if err == nil {
				output = append(output, map[string]any{"Build": value})
			}
		case "compaction-request":
			var value contextbuilder.Result
			value, err = current.BuildCompaction(step.Focus, step.Budget)
			if err == nil {
				output = append(output, map[string]any{"Build": value})
			}
		case "compact":
			made, ok := current.Compact(step.Response, step.Kept)
			output = append(output, map[string]any{"Compacted": made, "OK": ok})
		case "checkpoint":
			saved, err = current.(contextbuilder.Checkpointer).Checkpoint(step.Live)
			if err == nil {
				output = append(output, map[string]any{"Checkpoint": saved})
			}
		case "restart", "restore":
			if step.Action == "restart" {
				current = newBuilder()
			}
			state := saved
			if len(step.Encoded) != 0 {
				state = step.Encoded
			}
			err = current.(contextbuilder.Checkpointer).RestoreCheckpoint(state)
		case "notes":
			notes, notesError = step.Notes, step.NotesError
		case "notes-update":
			text, changed := current.NotesUpdate()
			output = append(output, map[string]any{"NotesUpdate": text, "Changed": changed})
		default:
			err = fmt.Errorf("unknown context action %q", step.Action)
		}
		if err != nil {
			output = append(output, map[string]any{"Error": err.Error()})
		}
	}
	return output
}
