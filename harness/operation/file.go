package operation

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/fileops"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
	"github.com/gfhfyjbr/kou-conveyor/harness/transcript"
)

// A file operation reads or changes a file as the file tools do (Read, Edit,
// Write, apply_patch): the work runs on a compute primitive and its result,
// written for the model, is the operation's state.

const (
	TypeFile    Type    = "file"
	VersionFile Version = 1

	fileComputeCorrelation primitives.CorrelationID = "file"
)

type FileAction string

const (
	FileRead  FileAction = "read"
	FileEdit  FileAction = "edit"
	FileWrite FileAction = "write"
	FilePatch FileAction = "patch"
	// FileTranscriptSearch searches the session's transcript at Path for
	// Query, a regular expression, and shows up to Limit matches.
	FileTranscriptSearch FileAction = "transcript_search"
)

// FileState is the request of a file operation and, once done, its result.
type FileState struct {
	Action FileAction
	// Path is the file, absolute; a patch names its own files, relative
	// to Root.
	Path string `json:",omitzero"`
	Root string `json:",omitzero"`
	// Offset and Limit are the lines a read shows.
	Offset int `json:",omitzero"`
	Limit  int `json:",omitzero"`
	// Content is what a write puts in the file.
	Content string `json:",omitzero"`
	// OldString, NewString and ReplaceAll are what an edit changes.
	OldString  string `json:",omitzero"`
	NewString  string `json:",omitzero"`
	ReplaceAll bool   `json:",omitzero"`
	// Patch is what a patch applies.
	Patch string `json:",omitzero"`
	// Query is what a transcript search looks for.
	Query string `json:",omitzero"`

	Result        *FileResult `json:",omitzero"`
	TerminalError string      `json:",omitzero"`
}

// FileResult is what a file operation did: the text the model reads, and
// what the cockpits show of it.
type FileResult struct {
	Text string
	// Files are the files the operation changed.
	Files []string `json:",omitzero"`
	// Lines, From, To and Truncated describe what a read showed.
	Lines     int  `json:",omitzero"`
	From      int  `json:",omitzero"`
	To        int  `json:",omitzero"`
	Truncated bool `json:",omitzero"`
}

func NewFileSpec(state FileState) (Spec, error) {
	if err := validateFileState(state); err != nil {
		return Spec{}, err
	}
	encoded, err := json.Marshal(state)
	if err != nil {
		return Spec{}, fmt.Errorf("encode file operation state: %w", err)
	}
	return Spec{Type: TypeFile, Version: VersionFile, State: encoded}, nil
}

func validateFileState(state FileState) error {
	switch state.Action {
	case FileRead, FileEdit, FileWrite:
		if state.Path == "" {
			return errors.New("file path must be set")
		}
	case FilePatch:
		if strings.TrimSpace(state.Patch) == "" {
			return errors.New("patch must be set")
		}
	case FileTranscriptSearch:
		if state.Path == "" {
			return errors.New("transcript path must be set")
		}
		if strings.TrimSpace(state.Query) == "" {
			return errors.New("query must be set")
		}
	default:
		return fmt.Errorf("unsupported file action %q", state.Action)
	}
	if state.Offset < 0 || state.Limit < 0 {
		return errors.New("offset and limit must not be negative")
	}
	return nil
}

func DecodeFileState(current Operation) (FileState, error) {
	if current.Type != TypeFile || current.Version != VersionFile {
		return FileState{}, fmt.Errorf(
			"decode file operation %q: type %q version %d: %w",
			current.ID, current.Type, current.Version, ErrUnsupported,
		)
	}
	var state FileState
	if err := json.Unmarshal(current.State, &state); err != nil {
		return FileState{}, fmt.Errorf("decode file operation %q state: %w", current.ID, err)
	}
	if err := validateFileState(state); err != nil {
		return FileState{}, fmt.Errorf("validate file operation %q state: %w", current.ID, err)
	}
	return state, nil
}

// AdvanceFile runs a file operation: it computes the result, once.
func AdvanceFile(current Operation, event *primitives.PrimitiveEvent) (Step, error) {
	state, err := DecodeFileState(current)
	if err != nil {
		return Step{}, err
	}
	switch current.Status {
	case StatusReady:
		if event != nil {
			return Step{}, fmt.Errorf("advance ready file operation %q: unexpected primitive event", current.ID)
		}
		if state.Result != nil || state.TerminalError != "" {
			return Step{}, fmt.Errorf("advance ready file operation %q: state is not initial", current.ID)
		}
		return dispatchFile(current, state)
	case StatusAwaiting:
		if event == nil {
			// Resumed after a restart. A read, a search or a write is the
			// same done twice, so it is done again; an edit or a patch may
			// have been made (its old text gone, or held in its new text,
			// which would take it twice), so the model looks first.
			switch state.Action {
			case FileEdit:
				return finishFile(current, state, StatusFailed, nil, "the edit was interrupted by a restart before its result was recorded; read the file to see whether it was made before making it again")
			case FilePatch:
				return finishFile(current, state, StatusFailed, nil, "the patch was interrupted by a restart before its result was recorded; read the files to see whether it was applied before applying it again")
			}
			return dispatchFile(current, state)
		}
		return fileEvent(current, state, *event)
	case StatusCanceling:
		if event != nil {
			return Step{}, fmt.Errorf("advance canceling file operation %q: unexpected primitive event", current.ID)
		}
		return finishFile(current, state, StatusCanceled, nil, "file operation canceled")
	default:
		return Step{}, fmt.Errorf("advance file operation %q: terminal status %q", current.ID, current.Status)
	}
}

func dispatchFile(current Operation, state FileState) (Step, error) {
	current.Status = StatusAwaiting
	step, err := fileStep(current, state)
	if err != nil {
		return Step{}, err
	}
	step.Dispatches = []PrimitiveDispatch{{
		Type: primitives.PrimitiveDispatchCompute,
		Data: primitives.ComputeRequest{
			Source: primitives.SourceID(current.ID), CorrelationID: fileComputeCorrelation,
			Run: func(context.Context) (any, error) {
				// A failure's message is what the model reads.
				return RunFile(state)
			},
		},
	}}
	return step, nil
}

// RunFile does the work of a file operation, now, and returns what the
// model reads of it.
func RunFile(state FileState) (FileResult, error) {
	switch state.Action {
	case FileRead:
		read, err := fileops.Read(state.Path, fileops.ReadOptions{Offset: state.Offset, Limit: state.Limit})
		if err != nil {
			return FileResult{}, err
		}
		return FileResult{Text: read.Text, Lines: read.Lines, From: read.From, To: read.To, Truncated: read.Truncated}, nil
	case FileEdit:
		edited, err := fileops.Edit(state.Path, state.OldString, state.NewString, fileops.EditOptions{ReplaceAll: state.ReplaceAll})
		if err != nil {
			return FileResult{}, err
		}
		var text strings.Builder
		if edited.Replacements == 1 {
			fmt.Fprintf(&text, "Edited %s: replaced 1 occurrence at line %d.", state.Path, edited.Line)
		} else {
			fmt.Fprintf(&text, "Edited %s: replaced %d occurrences, the first at line %d.", state.Path, edited.Replacements, edited.Line)
		}
		if edited.Snippet != "" {
			text.WriteString(" The file now reads around the first change:\n" + edited.Snippet)
		}
		return FileResult{Text: text.String(), Files: []string{state.Path}, Lines: edited.Lines}, nil
	case FileWrite:
		written, err := fileops.Write(state.Path, state.Content)
		if err != nil {
			return FileResult{}, err
		}
		text := fmt.Sprintf("Wrote %s: %s, %d bytes.", state.Path, lines(written.Lines), written.Bytes)
		if !written.Created {
			text = fmt.Sprintf("Replaced %s (it had %s): now %s, %d bytes.", state.Path, lines(written.PreviousLines), lines(written.Lines), written.Bytes)
		}
		return FileResult{Text: text, Files: []string{state.Path}, Lines: written.Lines}, nil
	case FilePatch:
		applied, err := fileops.ApplyPatch(state.Patch, state.Root)
		if err != nil {
			return FileResult{}, err
		}
		return FileResult{Text: applied.Summary(), Files: applied.Files()}, nil
	case FileTranscriptSearch:
		text, err := transcript.Search(state.Path, state.Query, state.Limit)
		if err != nil {
			return FileResult{}, err
		}
		return FileResult{Text: text}, nil
	}
	return FileResult{}, fmt.Errorf("unsupported file action %q", state.Action)
}

func lines(n int) string {
	if n == 1 {
		return "1 line"
	}
	return fmt.Sprintf("%d lines", n)
}

func fileEvent(current Operation, state FileState, event primitives.PrimitiveEvent) (Step, error) {
	if event.Source != primitives.SourceID(current.ID) {
		return finishFile(current, state, StatusFailed, nil, fmt.Sprintf("file primitive event source is %q, want %q", event.Source, current.ID))
	}
	switch event.Type {
	case primitives.PrimitiveEventCanceled:
		return finishFile(current, state, StatusCanceled, nil, "file operation canceled")
	case primitives.PrimitiveEventFailed:
		failure, ok := event.Result.(primitives.PrimitiveFailureResult)
		message := "file operation failed with an invalid result"
		if ok {
			message = failure.Error
		}
		return finishFile(current, state, StatusFailed, nil, message)
	case primitives.PrimitiveEventComputeCompleted:
		computed, ok := event.Result.(primitives.ComputeResult)
		if !ok {
			return finishFile(current, state, StatusFailed, nil, "file operation returned an invalid compute result")
		}
		result, ok := computed.Value.(FileResult)
		if !ok {
			return finishFile(current, state, StatusFailed, nil, "file operation returned an invalid result")
		}
		return finishFile(current, state, StatusCompleted, &result, "")
	default:
		return finishFile(current, state, StatusFailed, nil, fmt.Sprintf("file operation returned unexpected event %q", event.Type))
	}
}

func finishFile(current Operation, state FileState, status Status, result *FileResult, terminalError string) (Step, error) {
	state.Result, state.TerminalError = result, terminalError
	current.Status = status
	return fileStep(current, state)
}

func fileStep(current Operation, state FileState) (Step, error) {
	encoded, err := json.Marshal(state)
	if err != nil {
		return Step{}, fmt.Errorf("encode file operation %q state: %w", current.ID, err)
	}
	current.State = encoded
	return Step{Operation: &current}, nil
}

func failFile(current Operation, cause error) (Operation, bool) {
	state, err := DecodeFileState(current)
	if err != nil {
		return Operation{}, false
	}
	step, err := finishFile(current, state, StatusFailed, nil, cause.Error())
	if err != nil {
		return Operation{}, false
	}
	return *step.Operation, true
}
