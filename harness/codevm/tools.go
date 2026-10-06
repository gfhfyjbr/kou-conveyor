package codevm

import (
	"fmt"
	"maps"
	"os"
	"slices"
	"sort"
	"strings"

	"github.com/gfhfyjbr/kou-conveyor/harness/transcript"
)

// SearchTranscript is transcriptSearch(): the matches of query in the
// transcript at path.
func SearchTranscript(path, query string, limit int) (string, error) {
	return transcript.Search(path, query, limit)
}

// argumentsDelimiter ends the here-document that carries a plugin tool's
// arguments, as the command tool's does.
const argumentsDelimiter = "KOU_CONVEYOR_TOOL_ARGUMENTS"

// commandScript is the shell script that runs a plugin's tool with the
// arguments on its standard input.
func commandScript(definition CommandTool, arguments string) string {
	var script strings.Builder
	script.WriteString("exec")
	for _, argument := range definition.Command {
		script.WriteString(" " + shellWord(argument))
	}
	fmt.Fprintf(&script, " <<'%s'\n%s\n%s\n", argumentsDelimiter, strings.ReplaceAll(arguments, "\n", " "), argumentsDelimiter)
	return script.String()
}

// commandEnvironment is the environment a plugin's tool runs with: the
// run's (the process's when the run has none of its own), and the tool's
// own variables, with the call's ID when there is one.
func commandEnvironment(base []string, definition CommandTool, callID string) []string {
	if base == nil {
		base = os.Environ()
	}
	environment := slices.Clone(base)
	variables := maps.Clone(definition.Environment)
	if variables == nil {
		variables = map[string]string{}
	}
	variables["KOU_CONVEYOR_TOOL_NAME"] = definition.Name
	if callID != "" {
		variables["KOU_CONVEYOR_TOOL_CALL_ID"] = callID
	}
	names := make([]string, 0, len(variables))
	for name := range variables {
		names = append(names, name)
	}
	sort.Strings(names)
	for _, name := range names {
		environment = append(environment, name+"="+variables[name])
	}
	return environment
}

func shellWord(word string) string {
	return "'" + strings.ReplaceAll(word, "'", `'\''`) + "'"
}
