package canvas

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"strconv"
	"strings"
	"time"
)

// Templates make text of values: {{name}} is replaced by the value of name,
// with the spaces inside the braces ignored; a name without a value is
// replaced by nothing. An edge's template makes the message from an output
// ({{text}}, {{title}}, {{data.<path>}}, {{from.title}}, {{from.id}},
// {{from.kind}}, {{edge.id}}, {{now}}); a preset's arguments, files and
// environment are templates of the node's ({{brief}}, {{files.<name>}},
// {{node.title}}, {{node.id}}, {{runtime.agent_session}},
// {{config.<key>}}).

// maxTemplate bounds an edge's template.
const maxTemplate = 8 << 10

// expand fills a template with lookup's values.
func expand(template string, lookup func(name string) (string, bool)) string {
	if !strings.Contains(template, "{{") {
		return template
	}
	var b strings.Builder
	rest := template
	for {
		start := strings.Index(rest, "{{")
		if start < 0 {
			b.WriteString(rest)
			break
		}
		end := strings.Index(rest[start+2:], "}}")
		if end < 0 {
			b.WriteString(rest)
			break
		}
		b.WriteString(rest[:start])
		name := strings.TrimSpace(rest[start+2 : start+2+end])
		if value, ok := lookup(name); ok {
			b.WriteString(value)
		}
		rest = rest[start+2+end+2:]
	}
	return b.String()
}

// placeholders reports whether a template names values.
func placeholders(template string) bool {
	start := strings.Index(template, "{{")
	return start >= 0 && strings.Contains(template[start:], "}}")
}

// messageVars are what an edge's template names.
type messageVars struct {
	text, title string
	data        jsontext.Value
	from        *Node
	edge        string
	now         time.Time
}

func (v messageVars) lookup(name string) (string, bool) {
	switch name {
	case "text":
		return v.text, true
	case "title":
		return v.title, true
	case "edge.id":
		return v.edge, true
	case "now":
		return v.now.Format(time.RFC3339), true
	case "from.title":
		if v.from != nil {
			return v.from.Title, true
		}
	case "from.id":
		if v.from != nil {
			return v.from.ID, true
		}
	case "from.kind":
		if v.from != nil {
			if v.from.Preset != "" && v.from.Kind == KindTerminal {
				return v.from.Preset, true
			}
			return v.from.Kind, true
		}
	case "data":
		if len(v.data) != 0 {
			return string(v.data), true
		}
	}
	if path, ok := strings.CutPrefix(name, "data."); ok && len(v.data) != 0 {
		var data any
		if json.Unmarshal(v.data, &data) != nil {
			return "", false
		}
		return lookupPath(data, path)
	}
	return "", false
}

// lookupPath finds a value in decoded JSON by a dotted path, array indices
// among its keys, and gives it as text: strings as they are, anything else
// as JSON.
func lookupPath(data any, path string) (string, bool) {
	current := data
	for key := range strings.SplitSeq(path, ".") {
		switch value := current.(type) {
		case map[string]any:
			next, ok := value[key]
			if !ok {
				return "", false
			}
			current = next
		case []any:
			index, err := strconv.Atoi(key)
			if err != nil || index < 0 || index >= len(value) {
				return "", false
			}
			current = value[index]
		default:
			return "", false
		}
	}
	return textOf(current), true
}

// textOf gives a decoded JSON value as text.
func textOf(value any) string {
	switch value := value.(type) {
	case nil:
		return ""
	case string:
		return value
	case float64:
		return strconv.FormatFloat(value, 'f', -1, 64)
	case bool:
		return strconv.FormatBool(value)
	}
	data, err := json.Marshal(value)
	if err != nil {
		return ""
	}
	return string(data)
}

// renderMessage makes a message's text: the edge's template, or the output's
// text, under a line that says where it comes from when header is set —
// the node's title and its ID, which an agent answers it by.
func renderMessage(template string, vars messageVars, header bool) string {
	if strings.TrimSpace(template) == "" {
		template = "{{text}}"
	}
	text := expand(template, vars.lookup)
	if header && vars.from != nil {
		text = headerOf("from", vars.from) + text
	}
	return text
}

// headerOf is the line a message to an agent starts with: [canvas] from
// «Title» (n_id):, or [canvas] reply from … for a reply.
func headerOf(what string, from *Node) string {
	return "[canvas] " + what + " «" + from.Title + "» (" + from.ID + "):\n"
}
