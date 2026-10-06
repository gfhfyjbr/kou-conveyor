// Package file gives the model the file tools: Read shows a file's lines
// numbered, Edit replaces a string in a file, Write replaces a file whole
// and apply_patch applies a patch in the format the GPT models write.
// Each call is a file operation; relative paths are resolved against the
// workspace.
package file

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"path/filepath"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

type Config struct {
	// Directory is the workspace, which relative paths are resolved
	// against.
	Directory string
	// Transcript is the session's transcript file, which TranscriptSearch
	// searches.
	Transcript string
}

// Kind is which of the file tools a translator is.
type Kind string

const (
	KindRead       Kind = "read"
	KindEdit       Kind = "edit"
	KindWrite      Kind = "write"
	KindApplyPatch Kind = "apply_patch"
	// KindTranscriptSearch searches the session's transcript.
	KindTranscriptSearch Kind = "transcript_search"
)

type translator struct {
	kind   Kind
	name   string
	config Config
	// reads is Read's journal (journal.go).
	reads *journal
}

// New returns the translator of one of the file tools; name is what the
// model calls it.
func New(kind Kind, name string, config Config) tool.Translator {
	current := &translator{kind: kind, name: name, config: config}
	if kind == KindRead {
		current.reads = newJournal()
	}
	return current
}

type arguments struct {
	Path       string  `json:"path"`
	FilePath   string  `json:"file_path"`
	Offset     int     `json:"offset"`
	Limit      int     `json:"limit"`
	Content    string  `json:"content"`
	OldString  *string `json:"old_string"`
	NewString  *string `json:"new_string"`
	ReplaceAll bool    `json:"replace_all"`
	Input      string  `json:"input"`
	Patch      string  `json:"patch"`
	Query      string  `json:"query"`
}

func (translator *translator) Translate(ctx tool.Context, call llm.ToolCall) tool.CallStatus {
	state, err := translator.state(call.Arguments)
	if err != nil {
		return tool.ErrorStatus(err.Error(), 0)
	}
	spec, err := operation.NewFileSpec(state)
	if err != nil {
		return tool.ErrorStatus(fmt.Sprintf("build %s operation: %v", translator.name, err), 0)
	}
	return tool.CallStatus{WaitingFor: []operation.ID{ctx.Submit(spec)}}
}

// state is the file operation a call asks for.
func (translator *translator) state(encoded string) (operation.FileState, error) {
	encoded = strings.TrimSpace(encoded)
	if encoded == "" {
		encoded = "{}"
	}
	if value := jsontext.Value(encoded); value.Kind() != '{' || !value.IsValid() {
		return operation.FileState{}, fmt.Errorf("%s arguments must be a JSON object", translator.name)
	}
	var parsed arguments
	if err := json.Unmarshal([]byte(encoded), &parsed); err != nil {
		return operation.FileState{}, fmt.Errorf("decode %s arguments: %v", translator.name, err)
	}
	path := strings.TrimSpace(parsed.Path)
	if path == "" {
		path = strings.TrimSpace(parsed.FilePath)
	}
	if strings.IndexByte(path, 0) >= 0 {
		return operation.FileState{}, fmt.Errorf(`%s argument "path" contains a NUL byte`, translator.name)
	}
	needPath := func() (string, error) {
		if path == "" {
			return "", fmt.Errorf(`%s argument "path" must be set`, translator.name)
		}
		return translator.resolve(path), nil
	}
	switch translator.kind {
	case KindRead:
		resolved, err := needPath()
		if err != nil {
			return operation.FileState{}, err
		}
		if parsed.Offset < 0 || parsed.Limit < 0 {
			return operation.FileState{}, fmt.Errorf(`%s arguments "offset" and "limit" must not be negative`, translator.name)
		}
		return operation.FileState{Action: operation.FileRead, Path: resolved, Offset: parsed.Offset, Limit: parsed.Limit}, nil
	case KindEdit:
		resolved, err := needPath()
		if err != nil {
			return operation.FileState{}, err
		}
		if parsed.OldString == nil || parsed.NewString == nil {
			return operation.FileState{}, fmt.Errorf(`%s arguments "old_string" and "new_string" must be set`, translator.name)
		}
		return operation.FileState{
			Action: operation.FileEdit, Path: resolved,
			OldString: *parsed.OldString, NewString: *parsed.NewString, ReplaceAll: parsed.ReplaceAll,
		}, nil
	case KindWrite:
		resolved, err := needPath()
		if err != nil {
			return operation.FileState{}, err
		}
		return operation.FileState{Action: operation.FileWrite, Path: resolved, Content: parsed.Content}, nil
	case KindApplyPatch:
		patch := parsed.Input
		if strings.TrimSpace(patch) == "" {
			patch = parsed.Patch
		}
		if strings.TrimSpace(patch) == "" {
			return operation.FileState{}, fmt.Errorf(`%s argument "input" must hold the patch`, translator.name)
		}
		return operation.FileState{Action: operation.FilePatch, Patch: patch, Root: translator.config.Directory}, nil
	case KindTranscriptSearch:
		if strings.TrimSpace(parsed.Query) == "" {
			return operation.FileState{}, fmt.Errorf(`%s argument "query" must be set`, translator.name)
		}
		if translator.config.Transcript == "" {
			return operation.FileState{}, errors.New("no transcript is available in this run")
		}
		if parsed.Limit < 0 {
			return operation.FileState{}, fmt.Errorf(`%s argument "limit" must not be negative`, translator.name)
		}
		return operation.FileState{Action: operation.FileTranscriptSearch, Path: translator.config.Transcript, Query: parsed.Query, Limit: parsed.Limit}, nil
	}
	return operation.FileState{}, fmt.Errorf("unsupported file tool kind %q", translator.kind)
}

func (translator *translator) resolve(path string) string {
	if translator.config.Directory != "" && !filepath.IsAbs(path) {
		// Preserve .. for the file system to resolve across symlinks.
		return translator.config.Directory + string(filepath.Separator) + path
	}
	return path
}

func (translator *translator) TranslateResult(callID string, status tool.CallStatus, operations []operation.Operation) (llm.ToolResult, error) {
	text := func(value string) llm.ToolResult {
		return llm.ToolResult{CallID: callID, Output: []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: value}}}
	}
	if status.Error != "" {
		if len(operations) != 0 {
			return llm.ToolResult{}, fmt.Errorf("%s call %q has both a validation error and operations", translator.name, callID)
		}
		return text("Error: " + status.Error), nil
	}
	if len(operations) != 1 {
		return llm.ToolResult{}, fmt.Errorf("%s call %q has %d operations, want 1", translator.name, callID, len(operations))
	}
	output, err := FormatResult(translator.name, callID, operations[0])
	if err != nil {
		return llm.ToolResult{}, err
	}
	if translator.reads != nil {
		output += translator.reads.note(callID, operations[0])
	}
	return text(output), nil
}

// FormatResult is what the model reads of a file operation.
func FormatResult(name, callID string, current operation.Operation) (string, error) {
	state, err := operation.DecodeFileState(current)
	if err != nil {
		return "", fmt.Errorf("decode %s call %q result: %w", name, callID, err)
	}
	switch current.Status {
	case operation.StatusReady, operation.StatusAwaiting, operation.StatusCanceling:
		return "The file operation is still running.", nil
	case operation.StatusCompleted:
		if state.Result == nil {
			return "", fmt.Errorf("%s call %q completed operation %q has no result", name, callID, current.ID)
		}
		return state.Result.Text, nil
	case operation.StatusFailed, operation.StatusCanceled:
		if state.TerminalError == "" {
			return "", errors.New("file operation " + string(current.Status) + " without an error")
		}
		return "Error: " + state.TerminalError, nil
	}
	return "", fmt.Errorf("%s call %q operation %q has invalid status %q", name, callID, current.ID, current.Status)
}
