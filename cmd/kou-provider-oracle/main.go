// Command kou-provider-oracle only dials its own httptest server. It never loads
// production credentials or dials an endpoint from input during migration.
package main

import (
	"bytes"
	"context"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/responsesapi"
	"github.com/gfhfyjbr/kou-conveyor/harness/primitives"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
func run() error {
	var request struct {
		Action, CacheKey string
		Stream           string
		ChunkSize        int
		Request          llm.Request
		Extensions       map[string]jsontext.Value
		Body             jsontext.Value
	}
	data, err := io.ReadAll(io.LimitReader(os.Stdin, 8<<20))
	if err != nil {
		return err
	}
	if err := json.Unmarshal(data, &request); err != nil {
		return err
	}
	if len(request.Action) >= len("anthropic-") && request.Action[:len("anthropic-")] == "anthropic-" {
		return runAnthropic(data)
	}
	if request.Action == "http" {
		return runHTTP(data)
	}
	if request.Action == "codex-config" {
		return runCodexConfig(data)
	}
	captured := make(chan []byte, 1)
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, incoming *http.Request) {
		body, err := io.ReadAll(io.LimitReader(incoming.Body, 8<<20))
		if err != nil {
			http.Error(writer, err.Error(), 500)
			return
		}
		captured <- body
		if request.Action == "stream" {
			writer.Header().Set("Content-Type", "text/event-stream")
			chunk := request.ChunkSize
			if chunk <= 0 {
				chunk = max(1, len(request.Stream))
			}
			for remaining := request.Stream; len(remaining) > 0; {
				n := min(chunk, len(remaining))
				if _, err := io.WriteString(writer, remaining[:n]); err != nil {
					return
				}
				remaining = remaining[n:]
				if flush, ok := writer.(http.Flusher); ok {
					flush.Flush()
				}
			}
			return
		}
		response := request.Body
		if len(response) == 0 {
			response = jsontext.Value(`{"id":"r","status":"completed","output":[]}`)
		}
		event, err := json.Marshal(struct {
			Type     string         `json:"type"`
			Response jsontext.Value `json:"response"`
		}{"response.completed", response})
		if err != nil {
			http.Error(writer, err.Error(), 500)
			return
		}
		event = bytes.ReplaceAll(bytes.ReplaceAll(event, []byte("\n"), []byte(" ")), []byte("\r"), []byte(" "))
		writer.Header().Set("Content-Type", "text/event-stream")
		_, _ = writer.Write(append(append([]byte("data: "), event...), '\n', '\n'))
	}))
	defer server.Close()
	remote := primitives.NewRemoteClientWithHTTPClient(server.Client())
	defer remote.Close()
	attempts := 1
	adapter, err := responsesapi.NewAdapter(remote, responsesapi.Config{Endpoint: server.URL + "/responses", Extensions: request.Extensions, MaxAttempts: &attempts, CacheKeyPlacement: responsesapi.CacheKeyPlacement{UsePromptCacheKeyField: true}})
	if err != nil {
		return err
	}
	ctx, cancel := context.WithTimeout(context.Background(), 3*time.Second)
	defer cancel()
	streamed := make([]llm.StreamEvent, 0)
	ctx = llm.WithStreamReporter(ctx, func(event llm.StreamEvent) { streamed = append(streamed, event) })
	response, err := adapter.Respond(ctx, request.Request, llm.RequestOptions{CacheKey: request.CacheKey})
	var result any
	message := ""
	if err != nil {
		message = err.Error()
	} else if request.Action == "request" {
		result = jsontext.Value(<-captured)
	} else if request.Action == "decode" {
		result = response
	} else if request.Action == "stream" {
		result = struct {
			Response llm.Response
			Stream   []llm.StreamEvent
		}{response, streamed}
	} else {
		return fmt.Errorf("unsupported oracle action %q", request.Action)
	}
	encoded, err := json.Marshal(struct {
		Result any
		Error  string
	}{result, message})
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(append(encoded, '\n'))
	return err
}
