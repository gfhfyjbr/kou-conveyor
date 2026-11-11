// Tool registry, alias, profile and output-bound projections for the Rust
// migration. Translators are inert labels; submitted specs are recorded, never run.
package main

import (
	"bufio"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"hash/fnv"
	"os"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

type input struct {
	Action     string
	Name       string
	Names      []string
	Models     []string
	Message    string
	Limit      int
	Encoded    string
	From       int
	To         int
	Enabled    []string
	Configured []configured
	Steps      []step
}

type configured struct {
	Name  string
	Waits bool
}

type step struct {
	Op         string
	Name       string
	Arguments  string
	Tool       llm.Tool
	Translator string
	Waits      bool
	Skill      tool.Skill
	Index      int
}

type output struct {
	Error       string
	Text        string
	Description string
	Names       []string
	Resolved    []string
	Definitions string
	Found       bool
	Status      *tool.CallStatus
	Specs       []operation.Spec
	Result      *llm.ToolResult
	ResultError string
	Waits       bool
	Skills      []tool.Skill
	Limit       int
	Steps       []output
}

// labelled says which translator a name resolved to without doing anything.
type labelled struct {
	label string
	waits bool
}

func (translator labelled) Translate(_ tool.Context, call llm.ToolCall) tool.CallStatus {
	return tool.CallStatus{Error: "translated by " + translator.label + " as " + call.Name}
}

func (translator labelled) TranslateResult(callID string, _ tool.CallStatus, _ []operation.Operation) (llm.ToolResult, error) {
	return llm.ToolResult{CallID: callID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: "result of " + translator.label}}}, nil
}

func (translator labelled) Waits() bool { return translator.waits }

type recording struct{ specs []operation.Spec }

func (ctx *recording) Submit(spec operation.Spec) operation.ID {
	ctx.specs = append(ctx.specs, spec)
	return operation.ID(fmt.Sprintf("submitted-%d", len(ctx.specs)))
}

func names(registry tool.Registry) []string {
	var names []string
	for _, definition := range registry.StaticDefinitions() {
		names = append(names, definition.Tool.Name)
	}
	return names
}

func runRegistry(value input) output {
	var translators tool.StaticTranslators
	for _, slot := range value.Configured {
		translator := labelled{label: "static " + slot.Name, waits: slot.Waits}
		switch slot.Name {
		case tool.BashName:
			translators.Bash = translator
		case tool.ViewImageName:
			translators.ViewImage = translator
		case tool.ReadName:
			translators.Read = translator
		case tool.EditName:
			translators.Edit = translator
		case tool.WriteName:
			translators.Write = translator
		case tool.ApplyPatchName:
			translators.ApplyPatch = translator
		case tool.CodeName:
			translators.Code = translator
		case tool.TranscriptSearchName:
			translators.TranscriptSearch = translator
		default:
			return output{Error: fmt.Sprintf("unknown static slot %q", slot.Name)}
		}
	}
	registry := tool.NewRegistry(translators, value.Enabled...)
	ids := make([]tool.RegistrationID, len(value.Steps))
	var result output
	for index, current := range value.Steps {
		var part output
		switch current.Op {
		case "definitions":
			encoded, err := json.Marshal(registry.StaticDefinitions(), json.Deterministic(true))
			if err != nil {
				panic(err)
			}
			part.Definitions, part.Names = string(encoded), names(registry)
		case "resolve":
			translator, found := registry.Resolve(current.Name)
			part.Found = found
			if found {
				ctx := recording{}
				status := translator.Translate(&ctx, llm.ToolCall{CallID: "call", Name: current.Name, Arguments: current.Arguments})
				part.Status, part.Specs, part.Waits = &status, ctx.specs, tool.Waits(translator)
				translated, err := translator.TranslateResult("call", status, nil)
				if err != nil {
					part.ResultError = err.Error()
				} else {
					part.Result = &translated
				}
			}
		case "unavailable":
			part.Text = tool.Unavailable(registry, current.Name)
		case "register":
			var translator tool.Translator
			if current.Translator != "" {
				translator = labelled{label: current.Translator, waits: current.Waits}
			}
			if err := registry.RegisterTool(tool.Definition{Tool: current.Tool}, translator); err != nil {
				part.Error = err.Error()
			}
		case "register_skill":
			id, err := registry.RegisterSkill(current.Skill)
			if err != nil {
				part.Error = err.Error()
			}
			ids[index], part.Found = id, id != uuid.Nil()
		case "unregister_skill":
			switch {
			case current.Index >= 0:
				registry.UnregisterSkill(ids[current.Index])
			case current.Index == -1:
				registry.UnregisterSkill(uuid.Nil())
			default:
				registry.UnregisterSkill(uuid.New())
			}
		case "skills":
			part.Skills = registry.Skills()
		default:
			part.Error = fmt.Sprintf("unknown registry step %q", current.Op)
		}
		result.Steps = append(result.Steps, part)
	}
	return result
}

func describe(profile tool.Profile, models []string) output {
	result := output{Text: string(profile), Description: profile.Description(), Names: profile.Tools(), Found: profile.Valid()}
	for _, model := range models {
		result.Resolved = append(result.Resolved, string(profile.Resolve(model)))
	}
	return result
}

func parsed(name string) string {
	profile, err := tool.ParseProfile(name)
	if err != nil {
		return err.Error()
	}
	return string(profile)
}

func run(value input) output {
	switch value.Action {
	case "registry":
		return runRegistry(value)
	case "aliases":
		return output{Names: tool.Aliases(value.Name, value.Names)}
	case "profile":
		profile, err := tool.ParseProfile(value.Name)
		if err != nil {
			return output{Error: err.Error()}
		}
		return describe(profile, value.Models)
	case "models":
		var result output
		for _, model := range value.Models {
			result.Resolved = append(result.Resolved, string(tool.ProfileForModel(model)))
		}
		return result
	case "catalog":
		result := output{Text: tool.ProfileNames(), Names: tool.StaticNames(), Resolved: tool.AllProfileTools(), Limit: tool.DefaultShellTimeout}
		for _, profile := range tool.Profiles {
			result.Steps = append(result.Steps, describe(profile, value.Models))
		}
		return result
	case "error_status":
		status := tool.ErrorStatus(value.Message, value.Limit)
		return output{Status: &status}
	case "max_output_length":
		limit, err := tool.ParseMaxOutputLength(jsontext.Value(value.Encoded))
		if err != nil {
			// As the Bash translator, its one caller, reports it: the bare error
			// may cut a character in two, which JSON cannot carry.
			return output{Error: tool.ErrorStatus(err.Error(), 0).Error}
		}
		return output{Limit: limit}
	case "sweep":
		// Every scalar value through ParseProfile: Go's space, lower-case and
		// printable tables decide the outcome, and only its digest is compared.
		digest := fnv.New64a()
		for scalar := value.From; scalar < value.To; scalar++ {
			if scalar >= 0xd800 && scalar <= 0xdfff {
				continue
			}
			digest.Write([]byte(parsed(string(rune(scalar)))))
			digest.Write([]byte{0})
		}
		return output{Text: fmt.Sprintf("%016x", digest.Sum64())}
	default:
		return output{Error: fmt.Sprintf("unknown oracle action %q", value.Action)}
	}
}

func main() {
	scanner := bufio.NewScanner(os.Stdin)
	scanner.Buffer(make([]byte, 64<<10), 8<<20)
	writer := bufio.NewWriter(os.Stdout)
	defer writer.Flush()
	for scanner.Scan() {
		var value input
		if err := json.Unmarshal(scanner.Bytes(), &value); err != nil {
			panic(err)
		}
		encoded, err := json.Marshal(run(value))
		if err != nil {
			panic(err)
		}
		fmt.Fprintln(writer, string(encoded))
	}
	if err := scanner.Err(); err != nil {
		panic(err)
	}
}
