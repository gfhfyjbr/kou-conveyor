// Package transcript searches a session's transcript: the JSON Lines file
// the session store keeps, read as text so that nothing of it is needed
// here but the shape of its records. What the user wrote, what the model
// wrote, the tool calls it made and what the tools returned are searched;
// images and skills, which the tools show again, are not, nor the searches
// themselves.
package transcript

import (
	"bufio"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"hash/maphash"
	"io"
	"os"
	"regexp"
	"strings"
	"unicode/utf8"
)

const (
	DefaultLimit = 20
	MaxLimit     = 200
	// contextLines is how many lines each match shows around the line.
	contextLines = 2
	// matchLimit bounds the text of one match.
	matchLimit = 800
	// maxLine bounds a line of the transcript that is read; a longer one
	// carries an image or the like.
	maxLine = 64 << 20
)

// The searches are not searched: a search's query would find itself, and
// what it found is in the transcript already. They are the calls of the
// TranscriptSearch tool, by any name a gateway gave it, the file operations
// that ran them, and the transcriptSearch() calls of the Code tool.
const (
	searchName     = "TranscriptSearch"
	searchAction   = "transcript_search"
	codeSearchName = "transcriptSearch"
)

// searches reports a call of TranscriptSearch: a gateway may show the model
// the tool as "mcp__<server>__<word>_TranscriptSearch".
func searches(name string) bool {
	return name == searchName || strings.HasSuffix(name, "_"+searchName)
}

// entry is what a record of the transcript says, as text.
type entry struct {
	kind     string
	sequence int64
	text     string
}

// Search finds query, a case-insensitive regular expression, in the
// transcript at path and returns up to limit matches, each with the lines
// around it.
func Search(path, query string, limit int) (string, error) {
	query = strings.TrimSpace(query)
	if query == "" {
		return "", fmt.Errorf("the query must not be empty")
	}
	pattern, err := regexp.Compile("(?i)" + query)
	if err != nil {
		return "", fmt.Errorf("the query is not a valid regular expression: %v", err)
	}
	if limit <= 0 {
		limit = DefaultLimit
	}
	limit = min(limit, MaxLimit)
	file, err := os.Open(path)
	if err != nil {
		return "", fmt.Errorf("open the transcript: %v", err)
	}
	defer file.Close()

	var matches []string
	total := 0
	// A match that shows the same lines as one before it, as a Code call's
	// value and the output it came from do, is shown and counted once.
	seed, seen := maphash.MakeSeed(), map[uint64]bool{}
	reader := bufio.NewReaderSize(file, 1<<20)
	for {
		line, err := readLine(reader)
		if err != nil {
			if err == io.EOF {
				break
			}
			return "", fmt.Errorf("read the transcript: %v", err)
		}
		for _, current := range entries(line) {
			lines := strings.Split(current.text, "\n")
			for index, text := range lines {
				if !pattern.MatchString(text) {
					continue
				}
				header, body := formatMatch(current, lines, index)
				if key := maphash.String(seed, body); seen[key] {
					continue
				} else {
					seen[key] = true
				}
				total++
				if len(matches) < limit {
					matches = append(matches, header+body)
				}
			}
		}
	}
	if len(matches) == 0 {
		return fmt.Sprintf("No match for %q in the transcript.", query), nil
	}
	header := fmt.Sprintf("%d matches for %q in the transcript", total, query)
	if total > len(matches) {
		header += fmt.Sprintf(", the first %d shown", len(matches))
	}
	return header + ":\n\n" + strings.Join(matches, "\n\n"), nil
}

func readLine(reader *bufio.Reader) ([]byte, error) {
	var line []byte
	for {
		part, isPrefix, err := reader.ReadLine()
		if err != nil {
			return line, err
		}
		if len(line) < maxLine {
			line = append(line, part...)
		}
		if !isPrefix {
			return line, nil
		}
	}
}

// formatMatch is a match as the search shows it: a header naming where it
// is (an operation's result has no record number), and the lines around it.
func formatMatch(current entry, lines []string, at int) (string, string) {
	from, to := max(0, at-contextLines), min(len(lines)-1, at+contextLines)
	header := fmt.Sprintf("[%s, line %d]\n", current.kind, at+1)
	if current.sequence != 0 {
		header = fmt.Sprintf("[%s · record %d, line %d]\n", current.kind, current.sequence, at+1)
	}
	var text strings.Builder
	for index := from; index <= to; index++ {
		mark := "  "
		if index == at {
			mark = "> "
		}
		text.WriteString(mark + lines[index] + "\n")
	}
	body := strings.TrimRight(text.String(), "\n")
	if len(body) > matchLimit {
		cut := matchLimit
		for cut > 0 && !utf8.RuneStart(body[cut]) {
			cut--
		}
		body = body[:cut] + "…"
	}
	return header, body
}

// entries reads what a record says. A record is {"type": ..., "data": ...};
// an item's data is {"Item": {"Sequence", "Kind", "Data"}}, an operation's
// {"Operation": {...}}.
func entries(line []byte) []entry {
	var record struct {
		Type string         `json:"type"`
		Data jsontext.Value `json:"data"`
	}
	if json.Unmarshal(line, &record) != nil {
		return nil
	}
	switch record.Type {
	case "item":
		var wrapper struct {
			Item struct {
				Sequence int64          `json:"Sequence"`
				Kind     string         `json:"Kind"`
				Data     jsontext.Value `json:"Data"`
			} `json:"Item"`
		}
		if json.Unmarshal(record.Data, &wrapper) != nil {
			return nil
		}
		item := wrapper.Item
		switch item.Kind {
		case "input":
			var input struct {
				Kind    string         `json:"Kind"`
				Payload jsontext.Value `json:"Payload"`
			}
			if json.Unmarshal(item.Data, &input) != nil || input.Kind != "external" {
				return nil
			}
			var text string
			if json.Unmarshal(input.Payload, &text) != nil {
				var message struct {
					Text string `json:"Text"`
				}
				json.Unmarshal(input.Payload, &message)
				text = message.Text
			}
			if strings.TrimSpace(text) == "" {
				return nil
			}
			return []entry{{kind: "user", sequence: item.Sequence, text: text}}
		case "model_response":
			var response struct {
				Response struct {
					Output []struct {
						Type string         `json:"Type"`
						Data jsontext.Value `json:"Data"`
					} `json:"Output"`
				} `json:"Response"`
			}
			if json.Unmarshal(item.Data, &response) != nil {
				return nil
			}
			var found []entry
			for _, output := range response.Response.Output {
				switch output.Type {
				case "message":
					var message struct {
						Role string `json:"Role"`
						Text string `json:"Text"`
					}
					if json.Unmarshal(output.Data, &message) == nil && strings.TrimSpace(message.Text) != "" {
						found = append(found, entry{kind: "assistant", sequence: item.Sequence, text: message.Text})
					}
				case "tool_call":
					var call struct {
						Name      string `json:"Name"`
						Arguments string `json:"Arguments"`
					}
					if json.Unmarshal(output.Data, &call) == nil && !searches(call.Name) {
						found = append(found, entry{kind: "tool call " + call.Name, sequence: item.Sequence, text: call.Arguments})
					}
				}
			}
			return found
		}
	case "operation":
		var wrapper struct {
			Operation struct {
				Type   string         `json:"Type"`
				Status string         `json:"Status"`
				State  jsontext.Value `json:"State"`
			} `json:"Operation"`
		}
		if json.Unmarshal(record.Data, &wrapper) != nil {
			return nil
		}
		operation := wrapper.Operation
		if operation.Status != "completed" && operation.Status != "failed" {
			return nil
		}
		var state map[string]any
		if json.Unmarshal(operation.State, &state) != nil || state["Action"] == searchAction {
			return nil
		}
		var parts []string
		if result, ok := state["Result"]; ok {
			parts = append(parts, strings.Join(stringsOf(result, 0), "\n"))
		}
		if failure, ok := state["TerminalError"].(string); ok && failure != "" {
			parts = append(parts, "Error: "+failure)
		}
		text := strings.TrimSpace(strings.Join(parts, "\n"))
		if text == "" {
			return nil
		}
		return []entry{{kind: "tool result " + operation.Type, text: text}}
	}
	return nil
}

// stringsOf collects the text a result holds, but not images, skills or
// the like, which the tools show again.
func stringsOf(value any, depth int) []string {
	if depth > 4 {
		return nil
	}
	switch value := value.(type) {
	case string:
		return []string{value}
	case map[string]any:
		var found []string
		for _, key := range []string{"Out", "Err", "Text", "Value", "Logs", "Error", "Output"} {
			if text, ok := value[key].(string); ok && text != "" {
				found = append(found, text)
			}
		}
		if calls, ok := value["Calls"].([]any); ok {
			for _, call := range calls {
				if fields, ok := call.(map[string]any); ok && fields["Name"] == codeSearchName {
					continue
				}
				found = append(found, stringsOf(call, depth+1)...)
			}
		}
		return found
	case []any:
		var found []string
		for _, element := range value {
			found = append(found, stringsOf(element, depth+1)...)
		}
		return found
	}
	return nil
}
