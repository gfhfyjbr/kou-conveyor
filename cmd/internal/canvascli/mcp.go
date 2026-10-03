package canvascli

import (
	"bufio"
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"slices"
	"strings"
	"sync"
)

// kou-canvas mcp serves the canvas's tools to Claude Code and Codex as an
// MCP server: JSON-RPC 2.0 over stdio, a message a line. It does what the
// protocol needs for tools — initialize, tools/list, tools/call, ping —
// and calls each tool as kou-canvas tool does.

// mcpVersions are the protocol versions the server speaks, the latest
// first.
var mcpVersions = []string{"2025-06-18", "2025-03-26", "2024-11-05"}

const mcpInstructions = "These tools act on the kou-conveyor canvas this program runs on: a board of terminals, coding agents and event sources wired output to input. " +
	"To message a node, one call does it: CanvasSend with the node's ID or title, and wait: true to have its answer as the result. " +
	"Call CanvasView before you create, connect or remove anything; wait with CanvasSend's or CanvasRead's wait instead of polling."

type rpcMessage struct {
	JSONRPC string         `json:"jsonrpc"`
	ID      jsontext.Value `json:"id,omitzero"`
	Method  string         `json:"method,omitzero"`
	Params  jsontext.Value `json:"params,omitzero"`
}

type rpcError struct {
	Code    int    `json:"code"`
	Message string `json:"message"`
}

type mcpServer struct {
	a  *app
	mu sync.Mutex // one message written at a time
	// calls are the tool calls going on, by their request's ID, to cancel.
	calls map[string]context.CancelFunc
	wg    sync.WaitGroup
}

func (a *app) mcp(ctx context.Context) int {
	s := &mcpServer{a: a, calls: map[string]context.CancelFunc{}}
	reader := bufio.NewReader(a.in)
	for {
		line, err := reader.ReadBytes('\n')
		if line = bytes.TrimSpace(line); len(line) != 0 {
			s.handle(ctx, line)
		}
		if err != nil {
			break
		}
	}
	s.wg.Wait()
	return 0
}

func (s *mcpServer) write(v any) {
	data, err := json.Marshal(v)
	if err != nil {
		return
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	s.a.out.Write(append(data, '\n'))
}

func (s *mcpServer) reply(id jsontext.Value, result any) {
	s.write(map[string]any{"jsonrpc": "2.0", "id": id, "result": result})
}

func (s *mcpServer) fail(id jsontext.Value, code int, message string) {
	s.write(map[string]any{"jsonrpc": "2.0", "id": id, "error": rpcError{code, message}})
}

func (s *mcpServer) handle(ctx context.Context, line []byte) {
	var m rpcMessage
	if err := json.Unmarshal(line, &m); err != nil {
		s.fail(jsontext.Value("null"), -32700, "parse error")
		return
	}
	notification := len(m.ID) == 0
	switch m.Method {
	case "initialize":
		var params struct {
			ProtocolVersion string `json:"protocolVersion"`
		}
		_ = json.Unmarshal(m.Params, &params)
		version := mcpVersions[0]
		if slices.Contains(mcpVersions, params.ProtocolVersion) {
			version = params.ProtocolVersion
		}
		s.reply(m.ID, map[string]any{
			"protocolVersion": version,
			"capabilities":    map[string]any{"tools": map[string]any{"listChanged": false}},
			"serverInfo":      map[string]any{"name": "kou-canvas", "version": "1.0.0"},
			"instructions":    mcpInstructions,
		})
	case "ping":
		if !notification {
			s.reply(m.ID, map[string]any{})
		}
	case "tools/list":
		list := make([]map[string]any, 0, len(tools))
		for _, t := range tools {
			list = append(list, map[string]any{"name": t.Name, "description": t.Description, "inputSchema": jsontext.Value(t.Schema)})
		}
		s.reply(m.ID, map[string]any{"tools": list})
	case "tools/call":
		var params struct {
			Name      string         `json:"name"`
			Arguments jsontext.Value `json:"arguments"`
		}
		if err := json.Unmarshal(m.Params, &params); err != nil {
			s.fail(m.ID, -32602, "invalid params: "+err.Error())
			return
		}
		t, ok := toolNamed(params.Name)
		if !ok {
			s.fail(m.ID, -32602, fmt.Sprintf("no tool %q", params.Name))
			return
		}
		// A call may wait long (CanvasRead's wait): the others go on.
		ctx, cancel := context.WithCancel(ctx)
		key := string(m.ID)
		s.mu.Lock()
		s.calls[key] = cancel
		s.mu.Unlock()
		s.wg.Add(1)
		go func() {
			defer s.wg.Done()
			defer func() {
				s.mu.Lock()
				delete(s.calls, key)
				s.mu.Unlock()
				cancel()
			}()
			text, isError := s.call(ctx, t, params.Arguments)
			s.reply(m.ID, map[string]any{"content": []map[string]any{{"type": "text", "text": text}}, "isError": isError})
		}()
	case "notifications/cancelled":
		var params struct {
			RequestID jsontext.Value `json:"requestId"`
		}
		if json.Unmarshal(m.Params, &params) == nil {
			s.mu.Lock()
			if cancel := s.calls[string(params.RequestID)]; cancel != nil {
				cancel()
			}
			s.mu.Unlock()
		}
	default:
		if !notification && !strings.HasPrefix(m.Method, "notifications/") {
			s.fail(m.ID, -32601, "method not found: "+m.Method)
		}
	}
}

// call runs a tool for the MCP client.
func (s *mcpServer) call(ctx context.Context, t Tool, args jsontext.Value) (string, bool) {
	c, err := s.a.canvasClient()
	if err != nil {
		return err.Error(), true
	}
	res, err := t.call(ctx, c, args)
	if err != nil {
		return err.Error(), true
	}
	return strings.TrimRight(res.text, "\n"), false
}
