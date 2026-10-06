package codevm

import (
	"fmt"
	"sort"
	"strings"
)

// Declarations is the API of a run as the model reads it: TypeScript
// declarations of the functions the code may call, the plugins' tools
// among them, with what each returns.
func Declarations(config Config) string {
	var text strings.Builder
	text.WriteString(`// Run a shell command (bash -c) in the workspace. Resolves when it exits, whatever the exit code: check exitCode.
// options.timeout is in seconds (default 600; 0 for none but the run's own); options.maxOutputLength bounds each stream (default 40000).
declare function bash(command: string, options?: { timeout?: number; maxOutputLength?: number }): Promise<{ stdout: string; stderr: string; exitCode: number; timedOut: boolean }>;
// A file's lines numbered like cat -n, from line offset (1) up to limit lines (2000) and about 40000 bytes: what you would quote.
declare function read(path: string, options?: { offset?: number; limit?: number }): Promise<string>;
// A file's whole text, as it is, to process in code (up to 4 MB).
declare function readText(path: string): Promise<string>;
// Create a file or replace it whole, creating directories. Resolves to a short report.
declare function write(path: string, content: string): Promise<string>;
// Replace oldString, which must match the file exactly and once (or every time with replaceAll), with newString.
// Resolves to a report with the lines around the change; rejects when oldString is missing or ambiguous.
declare function edit(path: string, oldString: string, newString: string, options?: { replaceAll?: boolean }): Promise<string>;
// Apply a patch in the apply_patch format (*** Begin Patch … *** End Patch). Rejects, changing nothing, when it does not apply.
declare function applyPatch(patch: string): Promise<string>;
// Attach an image (PNG, JPEG, GIF, WebP, BMP, TIFF) to the result, for you to see.
declare function viewImage(path: string): Promise<string>;
// Search the session's whole transcript (including what was compacted or pruned) with a regular expression.
declare function transcriptSearch(query: string, options?: { limit?: number }): Promise<string>;
// Wait for a while.
declare function sleep(milliseconds: number): Promise<void>;
// console.log lines are reported with the result.
declare const console: { log(...values: unknown[]): void; error(...values: unknown[]): void };
`)
	if len(config.Skills) != 0 {
		names := make([]string, 0, len(config.Skills))
		for name := range config.Skills {
			names = append(names, name)
		}
		sort.Strings(names)
		quoted := make([]string, len(names))
		for index, name := range names {
			quoted[index] = fmt.Sprintf("%q", name)
		}
		fmt.Fprintf(&text, "// Load the instructions of a skill: the text of its SKILL.md.\ndeclare function skill(name: %s): Promise<string>;\n", strings.Join(quoted, " | "))
	}
	if len(config.Tools) != 0 {
		text.WriteString("\n// The plugins' tools: each takes one object of arguments, resolves to what it prints and rejects when it fails (a nonzero exit). Also as tools[name](args).\n")
		for _, definition := range config.Tools {
			if definition.Description != "" {
				for _, line := range strings.Split(strings.TrimSpace(definition.Description), "\n") {
					text.WriteString("// " + line + "\n")
				}
			}
			name := definition.Name
			if !bindable(name) {
				name = "tools[" + fmt.Sprintf("%q", name) + "]"
				fmt.Fprintf(&text, "// %s(args: %s): Promise<string>\n", name, typeOf(definition.Parameters, 0))
				continue
			}
			fmt.Fprintf(&text, "declare function %s(args: %s): Promise<string>;\n", name, typeOf(definition.Parameters, 0))
		}
	}
	return strings.TrimRight(text.String(), "\n")
}

// typeOf is the TypeScript type of a JSON schema.
func typeOf(schema map[string]any, depth int) string {
	if schema == nil {
		return "Record<string, unknown>"
	}
	if values, ok := schema["enum"].([]any); ok && len(values) != 0 {
		parts := make([]string, len(values))
		for index, value := range values {
			if text, ok := value.(string); ok {
				parts[index] = fmt.Sprintf("%q", text)
			} else {
				parts[index] = fmt.Sprint(value)
			}
		}
		return strings.Join(parts, " | ")
	}
	kind, _ := schema["type"].(string)
	if kinds, ok := schema["type"].([]any); ok && len(kinds) != 0 {
		parts := make([]string, 0, len(kinds))
		for _, element := range kinds {
			if text, ok := element.(string); ok {
				parts = append(parts, typeOf(map[string]any{"type": text}, depth))
			}
		}
		return strings.Join(parts, " | ")
	}
	switch kind {
	case "string":
		return "string"
	case "number", "integer":
		return "number"
	case "boolean":
		return "boolean"
	case "null":
		return "null"
	case "array":
		items, _ := schema["items"].(map[string]any)
		element := typeOf(items, depth+1)
		if strings.Contains(element, "|") || strings.Contains(element, " ") {
			element = "(" + element + ")"
		}
		return element + "[]"
	case "object", "":
		properties, _ := schema["properties"].(map[string]any)
		if len(properties) == 0 || depth > 6 {
			return "Record<string, unknown>"
		}
		required := map[string]bool{}
		if names, ok := schema["required"].([]any); ok {
			for _, name := range names {
				if text, ok := name.(string); ok {
					required[text] = true
				}
			}
		}
		names := make([]string, 0, len(properties))
		for name := range properties {
			names = append(names, name)
		}
		sort.Strings(names)
		parts := make([]string, 0, len(names))
		for _, name := range names {
			property, _ := properties[name].(map[string]any)
			key := name
			if !identifier.MatchString(name) {
				key = fmt.Sprintf("%q", name)
			}
			if !required[name] {
				key += "?"
			}
			part := key + ": " + typeOf(property, depth+1)
			if description, ok := property["description"].(string); ok && description != "" {
				part += " /* " + strings.Join(strings.Fields(description), " ") + " */"
			}
			parts = append(parts, part)
		}
		return "{ " + strings.Join(parts, "; ") + " }"
	}
	return "unknown"
}
