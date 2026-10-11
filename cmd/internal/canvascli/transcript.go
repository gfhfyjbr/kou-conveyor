package canvascli

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"net/url"
	"strconv"
	"strings"
)

// transcriptCursor accepts the byte cursor returned by CanvasTranscript. The
// server also accepts numeric JSON for callers that naturally represent a
// cursor as a number, while "last" asks the server to use its per-reader
// cursor.
type transcriptCursor string

func (c *transcriptCursor) UnmarshalJSON(data []byte) error {
	data = bytes.TrimSpace(data)
	if len(data) == 0 {
		return errors.New("it is a cursor or last")
	}
	if data[0] == '"' {
		var value string
		if err := json.Unmarshal(data, &value); err != nil {
			return err
		}
		*c = transcriptCursor(value)
		return nil
	}
	value := jsontext.Value(data)
	if value.Kind() == 'n' {
		*c = transcriptCursor(string(data))
		return nil
	}
	return errors.New("it is a cursor or last")
}

type transcriptArgs struct {
	Node         string           `json:"node"`
	After        transcriptCursor `json:"after"`
	Limit        number           `json:"limit"`
	Kinds        nodeList         `json:"kinds"`
	Tools        nodeList         `json:"tools"`
	Since        string           `json:"since"`
	Until        string           `json:"until"`
	MessageLimit number           `json:"message_limit"`
	OutputLimit  number           `json:"output_limit"`
	Include      nodeList         `json:"include"`
}

type searchArgs struct {
	Node  string `json:"node"`
	Nodes string `json:"nodes"`
	Query string `json:"q"`
	Limit number `json:"limit"`
}

func (c *client) transcript(ctx context.Context, a transcriptArgs) (result, error) {
	node := strings.TrimSpace(a.Node)
	if node == "" {
		return result{}, errors.New("name the node whose transcript to read")
	}
	q := url.Values{}
	if after := strings.TrimSpace(string(a.After)); after != "" {
		q.Set("after", after)
	}
	if value := intOf(a.Limit); value != nil {
		q.Set("limit", strconv.Itoa(*value))
	}
	if values := paths(a.Kinds); len(values) != 0 {
		q.Set("kinds", strings.Join(values, ","))
	}
	if values := paths(a.Tools); len(values) != 0 {
		q.Set("tools", strings.Join(values, ","))
	}
	if value := strings.TrimSpace(a.Since); value != "" {
		q.Set("since", value)
	}
	if value := strings.TrimSpace(a.Until); value != "" {
		q.Set("until", value)
	}
	if value := intOf(a.MessageLimit); value != nil {
		q.Set("message_limit", strconv.Itoa(*value))
	}
	if value := intOf(a.OutputLimit); value != nil {
		q.Set("output_limit", strconv.Itoa(*value))
	}
	if values := paths(a.Include); len(values) != 0 {
		q.Set("include", strings.Join(values, ","))
	}
	path := "/api/canvas/nodes/" + url.PathEscape(node) + "/transcript"
	raw, err := c.call(ctx, "GET", path, q, nil, &map[string]any{})
	if err != nil {
		return result{}, err
	}
	return result{text: prettyJSON(raw), raw: raw}, nil
}

func (c *client) search(ctx context.Context, a searchArgs) (result, error) {
	node := strings.TrimSpace(a.Node)
	if node == "" {
		node = "self"
	}
	query := strings.TrimSpace(a.Query)
	if query == "" {
		return result{}, errors.New("name a search query with q")
	}
	q := url.Values{"q": []string{query}}
	if nodes := strings.TrimSpace(a.Nodes); nodes != "" {
		q.Set("nodes", nodes)
	}
	if value := intOf(a.Limit); value != nil {
		q.Set("limit", strconv.Itoa(*value))
	}
	path := "/api/canvas/nodes/" + url.PathEscape(node) + "/search"
	raw, err := c.call(ctx, "GET", path, q, nil, &map[string]any{})
	if err != nil {
		return result{}, err
	}
	return result{text: prettyJSON(raw), raw: raw}, nil
}

func prettyJSON(data []byte) string {
	value := jsontext.Value(bytes.TrimSpace(data)).Clone()
	if err := value.Indent(jsontext.WithIndentPrefix(""), jsontext.WithIndent("  ")); err == nil {
		return string(value)
	}
	return string(data)
}
