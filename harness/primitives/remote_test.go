package primitives

import (
	"bytes"
	"context"
	"crypto/x509"
	"errors"
	"fmt"
	"io"
	"net"
	"net/http"
	"net/http/httptest"
	"slices"
	"strings"
	"sync/atomic"
	"syscall"
	"testing"
	"testing/synctest"
	"time"
)

func TestDefaultRemoteRequest(t *testing.T) {
	request := DefaultRemoteRequest("source", "correlation", "https://example.com")

	if request.Source != "source" || request.CorrelationID != "correlation" {
		t.Fatalf("identity = (%q, %q)", request.Source, request.CorrelationID)
	}
	if request.Method != "GET" || request.URL != "https://example.com" {
		t.Fatalf("request = %s %s", request.Method, request.URL)
	}
	if request.Headers == nil || len(request.Headers) != 0 {
		t.Fatalf("headers = %#v, want empty map", request.Headers)
	}
	if request.ResponseIdleTimeout != 30*time.Second {
		t.Fatalf("response idle timeout = %s", request.ResponseIdleTimeout)
	}
	if request.SSE != nil {
		t.Fatalf("SSE options = %#v", request.SSE)
	}
	wantRetryPolicy := RemoteRetryPolicy{
		MaxAttempts:          5,
		InitialBackoff:       2 * time.Second,
		MaxBackoff:           30 * time.Second,
		RetryableStatusCodes: []int{408, 425, 429, 500, 502, 503, 504, 520, 521, 522, 523, 524, 529},
	}
	if request.RetryPolicy.MaxAttempts != wantRetryPolicy.MaxAttempts ||
		request.RetryPolicy.InitialBackoff != wantRetryPolicy.InitialBackoff ||
		request.RetryPolicy.MaxBackoff != wantRetryPolicy.MaxBackoff ||
		!slices.Equal(request.RetryPolicy.RetryableStatusCodes, wantRetryPolicy.RetryableStatusCodes) {
		t.Fatalf("retry policy = %#v", request.RetryPolicy)
	}
}

func TestNewRemoteHTTPClientOwnsTransport(t *testing.T) {
	client := newRemoteHTTPClient()
	defer client.CloseIdleConnections()

	if client.Transport == http.DefaultTransport {
		t.Fatal("client reuses http.DefaultTransport")
	}
	if _, ok := client.Transport.(*http.Transport); !ok {
		t.Fatalf("transport type = %T", client.Transport)
	}
}

func TestRemoteClientWrapsConfiguredHTTPClient(t *testing.T) {
	transport := &closeTrackingRemoteTransport{}
	httpClient := &http.Client{Transport: transport}
	client := NewRemoteClientWithHTTPClient(httpClient)

	if client.httpClient != httpClient {
		t.Fatal("remote client copied the configured HTTP client")
	}
	if err := client.Close(); err != nil {
		t.Fatal(err)
	}
	if transport.closeCalls.Load() != 1 {
		t.Fatalf("close idle connections calls = %d, want 1", transport.closeCalls.Load())
	}
}

func TestRemoteClientReusesConnectionsAcrossRequests(t *testing.T) {
	var connections atomic.Int32
	server := httptest.NewUnstartedServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.WriteHeader(http.StatusNoContent)
	}))
	server.Config.ConnState = func(connection net.Conn, state http.ConnState) {
		if state == http.StateNew {
			connections.Add(1)
		}
	}
	server.Start()
	defer server.Close()

	client := NewRemoteClient()
	defer func() {
		if err := client.Close(); err != nil {
			t.Errorf("close remote client: %v", err)
		}
	}()
	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.RetryPolicy.MaxAttempts = 1
	for range 2 {
		events := collectInternalEvents(sendRemoteRequest(client, t.Context(), request))
		if last := events[len(events)-1]; last.Type != PrimitiveEventRemoteCompleted {
			t.Fatalf("events = %#v", events)
		}
	}
	if connections.Load() != 1 {
		t.Fatalf("connections = %d, want one reused connection", connections.Load())
	}
}

func TestSendRemoteRequestStreamsResponse(t *testing.T) {
	body := bytes.Repeat([]byte("response body "), IOReadChunkSize/4)
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		if request.Method != http.MethodPost {
			t.Errorf("method = %q, want %q", request.Method, http.MethodPost)
		}
		if request.ContentLength != int64(len("request body")) {
			t.Errorf("content length = %d", request.ContentLength)
		}
		if got := request.Header.Values("X-Request"); len(got) != 2 || got[0] != "one" || got[1] != "two" {
			t.Errorf("X-Request = %#v", got)
		}
		requestBody, err := io.ReadAll(request.Body)
		if err != nil {
			t.Errorf("read request body: %v", err)
		}
		if err := request.Body.Close(); err != nil {
			t.Errorf("close request body: %v", err)
		}
		if string(requestBody) != "request body" {
			t.Errorf("request body = %q", requestBody)
		}

		writer.Header().Add("X-Response", "one")
		writer.Header().Add("X-Response", "two")
		writer.WriteHeader(http.StatusCreated)
		if _, err := writer.Write(body); err != nil {
			t.Errorf("write response: %v", err)
		}
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.Method = http.MethodPost
	request.Headers["X-Request"] = []string{"one", "two"}
	request.Body = []byte("request body")
	events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
	if len(events) < 3 {
		t.Fatalf("events = %#v", events)
	}
	assertRemoteEventIdentity(t, events, request)
	if events[0].Type != PrimitiveEventRemoteResponseStarted {
		t.Fatalf("first event type = %q", events[0].Type)
	}
	started := events[0].Result.(RemoteResponseStartedResult)
	if started.Attempt != 1 || started.StatusCode != http.StatusCreated {
		t.Fatalf("response started = %#v", started)
	}
	if got := started.Headers["X-Response"]; len(got) != 2 || got[0] != "one" || got[1] != "two" {
		t.Fatalf("X-Response = %#v", got)
	}

	var output []byte
	var offset int64
	for _, event := range events[1 : len(events)-1] {
		if event.Type != PrimitiveEventRemoteOutput {
			t.Fatalf("stream event type = %q", event.Type)
		}
		result := event.Result.(RemoteOutputResult)
		if result.Attempt != 1 || result.Offset != offset {
			t.Fatalf("output position = (%d, %d), want (1, %d)", result.Attempt, result.Offset, offset)
		}
		if len(result.Data) > IOReadChunkSize {
			t.Fatalf("output size = %d, exceeds %d", len(result.Data), IOReadChunkSize)
		}
		output = append(output, result.Data...)
		offset += int64(len(result.Data))
	}
	if !bytes.Equal(output, body) {
		t.Fatalf("output size = %d, want %d", len(output), len(body))
	}
	if last := events[len(events)-1]; last.Type != PrimitiveEventRemoteCompleted ||
		last.Result != (RemoteCompletedResult{Attempt: 1}) {
		t.Fatalf("last event = %#v", last)
	}
}

func TestSendRemoteRequestFramesSSEResponses(t *testing.T) {
	body := []byte("data: one\r\n\r\ndata: two\n\n")
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, _ *http.Request) {
		writer.Header().Set("Content-Type", "text/event-stream; charset=utf-8")
		if _, err := writer.Write(body); err != nil {
			t.Errorf("write response: %v", err)
		}
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.SSE = &RemoteSSEOptions{MaxFrameSize: int64(len(body))}
	events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
	outputs := remoteEventsOfType(events, PrimitiveEventRemoteOutput)
	want := [][]byte{[]byte("data: one"), []byte("data: two")}
	if len(outputs) != len(want) {
		t.Fatalf("output events = %#v", outputs)
	}
	var offset int64
	for index, event := range outputs {
		output := event.Result.(RemoteOutputResult)
		if output.Attempt != 1 || output.Offset != offset || !bytes.Equal(output.Data, want[index]) {
			t.Fatalf("output %d = %#v", index, output)
		}
		offset += int64(len(output.Data))
	}
	if last := events[len(events)-1]; last.Type != PrimitiveEventRemoteCompleted {
		t.Fatalf("last event = %#v", last)
	}
}

func TestSendRemoteRequestKeepsRawOutputForNonSSEResponses(t *testing.T) {
	body := []byte(`{"ok":true}`)
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, _ *http.Request) {
		writer.Header().Set("Content-Type", "application/json")
		if _, err := writer.Write(body); err != nil {
			t.Errorf("write response: %v", err)
		}
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.SSE = &RemoteSSEOptions{MaxFrameSize: 1}
	events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
	if output := remoteAttemptOutput(t, events, 1); !bytes.Equal(output, body) {
		t.Fatalf("output = %q", output)
	}
}

func TestSendRemoteRequestPreservesFramedOutputBeforeStreamFailure(t *testing.T) {
	tests := []struct {
		name string
		body []byte
	}{
		{name: "incomplete event", body: []byte("data: partial")},
		{name: "complete event and tail", body: []byte("data: complete\n\npartial")},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			body := &trackingRemoteBody{Reader: &terminalRemoteReader{
				data: test.body,
				err:  io.ErrUnexpectedEOF,
			}}
			client := &RemoteClient{httpClient: &http.Client{Transport: remoteRoundTripFunc(
				func(*http.Request) (*http.Response, error) {
					return &http.Response{
						StatusCode: http.StatusOK,
						Header:     http.Header{"Content-Type": {"text/event-stream"}},
						Body:       body,
					}, nil
				},
			)}}
			request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
			request.SSE = &RemoteSSEOptions{
				MaxFrameSize:   int64(len(test.body)),
				FrameDelimiter: SSEFrameDelimiterPassThrough,
			}
			request.RetryPolicy = RemoteRetryPolicy{MaxAttempts: 1}

			events := collectInternalEvents(sendRemoteRequest(client, t.Context(), request))
			assertRemoteEventTypes(t, events, []PrimitiveEventType{
				PrimitiveEventRemoteResponseStarted,
				PrimitiveEventRemoteOutput,
				PrimitiveEventRemoteStreamFailed,
				PrimitiveEventFailed,
			})
			if output := remoteAttemptOutput(t, events, 1); !bytes.Equal(output, test.body) {
				t.Fatalf("output = %q, want %q", output, test.body)
			}
			if !body.closed {
				t.Fatal("response body was not closed")
			}
		})
	}
}

func TestSendRemoteRequestReplaysBodyAcrossTemporaryRedirect(t *testing.T) {
	var requests atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		requests.Add(1)
		if request.Method != http.MethodPost {
			t.Errorf("method = %q, want POST", request.Method)
		}
		body, err := io.ReadAll(request.Body)
		if err != nil {
			t.Errorf("read request body: %v", err)
		}
		if err := request.Body.Close(); err != nil {
			t.Errorf("close request body: %v", err)
		}
		if string(body) != "replayed body" {
			t.Errorf("body = %q", body)
		}

		switch request.URL.Path {
		case "/start":
			writer.Header().Set("Location", "/target")
			writer.WriteHeader(http.StatusTemporaryRedirect)
		case "/target":
			writer.WriteHeader(http.StatusCreated)
			if _, err := writer.Write([]byte("redirect completed")); err != nil {
				t.Errorf("write response: %v", err)
			}
		default:
			t.Errorf("unexpected path %q", request.URL.Path)
			writer.WriteHeader(http.StatusNotFound)
		}
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL+"/start")
	request.Method = http.MethodPost
	request.Body = []byte("replayed body")
	request.RetryPolicy.MaxAttempts = 1
	events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))

	if requests.Load() != 2 {
		t.Fatalf("requests = %d, want redirect and target", requests.Load())
	}
	assertRemoteEventTypes(t, events, []PrimitiveEventType{
		PrimitiveEventRemoteResponseStarted,
		PrimitiveEventRemoteOutput,
		PrimitiveEventRemoteCompleted,
	})
	started := events[0].Result.(RemoteResponseStartedResult)
	if started.StatusCode != http.StatusCreated {
		t.Fatalf("status = %d", started.StatusCode)
	}
	if output := remoteAttemptOutput(t, events, 1); string(output) != "redirect completed" {
		t.Fatalf("output = %q", output)
	}
}

func TestSendRemoteRequestRetriesTransientResponse(t *testing.T) {
	for _, statusCode := range []int{http.StatusServiceUnavailable, 520, 521, 522, 523, 524, 529} {
		t.Run(fmt.Sprint(statusCode), func(t *testing.T) {
			var attempts atomic.Int32
			server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
				requestBody, err := io.ReadAll(request.Body)
				if err != nil {
					t.Errorf("read request body: %v", err)
				}
				if err := request.Body.Close(); err != nil {
					t.Errorf("close request body: %v", err)
				}
				if string(requestBody) != "replayed" {
					t.Errorf("request body = %q", requestBody)
				}

				attempt := attempts.Add(1)
				writer.Header().Set("X-Attempt", fmt.Sprint(attempt))
				if attempt == 1 {
					writer.WriteHeader(statusCode)
					if _, err := writer.Write([]byte("try later")); err != nil {
						t.Errorf("write retry response: %v", err)
					}
					return
				}
				writer.WriteHeader(http.StatusOK)
				if _, err := writer.Write([]byte("completed")); err != nil {
					t.Errorf("write response: %v", err)
				}
			}))
			defer server.Close()

			request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
			request.Method = http.MethodPost
			request.Body = []byte("replayed")
			request.RetryPolicy.MaxAttempts = 2
			request.RetryPolicy.InitialBackoff = 0
			request.RetryPolicy.MaxBackoff = 0
			events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
			if attempts.Load() != 2 {
				t.Fatalf("attempts = %d, want 2", attempts.Load())
			}
			assertRemoteEventTypes(t, events, []PrimitiveEventType{
				PrimitiveEventRemoteResponseStarted,
				PrimitiveEventRemoteOutput,
				PrimitiveEventRemoteRetryScheduled,
				PrimitiveEventRemoteResponseStarted,
				PrimitiveEventRemoteOutput,
				PrimitiveEventRemoteCompleted,
			})
			startedEvents := remoteEventsOfType(events, PrimitiveEventRemoteResponseStarted)
			firstStarted := startedEvents[0].Result.(RemoteResponseStartedResult)
			if firstStarted.Attempt != 1 || firstStarted.StatusCode != statusCode ||
				http.Header(firstStarted.Headers).Get("X-Attempt") != "1" {
				t.Fatalf("first response = %#v", firstStarted)
			}
			if output := remoteAttemptOutput(t, events, 1); string(output) != "try later" {
				t.Fatalf("first output = %q", output)
			}
			retry := remoteEventsOfType(events, PrimitiveEventRemoteRetryScheduled)[0].Result.(RemoteRetryScheduledResult)
			if retry.Attempt != 1 || retry.Delay != 0 || !strings.Contains(retry.Reason, fmt.Sprint(statusCode)) {
				t.Fatalf("retry = %#v", retry)
			}
			secondStarted := startedEvents[1].Result.(RemoteResponseStartedResult)
			if secondStarted.Attempt != 2 || secondStarted.StatusCode != http.StatusOK ||
				http.Header(secondStarted.Headers).Get("X-Attempt") != "2" {
				t.Fatalf("second response = %#v", secondStarted)
			}
			if output := remoteAttemptOutput(t, events, 2); string(output) != "completed" {
				t.Fatalf("second output = %q", output)
			}
			if completed := events[len(events)-1].Result.(RemoteCompletedResult); completed.Attempt != 2 {
				t.Fatalf("completed = %#v", completed)
			}
		})
	}
}

func TestRemoteRetryPolicyAcceptsAdditionalStatusCodes(t *testing.T) {
	policy := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com").RetryPolicy
	if isRetryableRemoteStatus(policy, http.StatusConflict) {
		t.Fatal("409 must not be retryable by default")
	}

	policy.RetryableStatusCodes = append(
		policy.RetryableStatusCodes,
		http.StatusConflict,
	)
	if !isRetryableRemoteStatus(policy, http.StatusConflict) {
		t.Fatal("409 must be retryable after being added")
	}
}

func TestSendRemoteRequestUsesAdditionalRetryableStatusCode(t *testing.T) {
	var attempts atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		if attempts.Add(1) == 1 {
			writer.WriteHeader(http.StatusConflict)
			return
		}
		writer.WriteHeader(http.StatusNoContent)
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.RetryPolicy.MaxAttempts = 2
	request.RetryPolicy.InitialBackoff = 0
	request.RetryPolicy.MaxBackoff = 0
	request.RetryPolicy.RetryableStatusCodes = append(
		request.RetryPolicy.RetryableStatusCodes,
		http.StatusConflict,
	)
	events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
	if attempts.Load() != 2 {
		t.Fatalf("attempts = %d, want 2", attempts.Load())
	}
	wants := []PrimitiveEventType{
		PrimitiveEventRemoteResponseStarted,
		PrimitiveEventRemoteRetryScheduled,
		PrimitiveEventRemoteResponseStarted,
		PrimitiveEventRemoteCompleted,
	}
	if len(events) != len(wants) {
		t.Fatalf("events = %#v", events)
	}
	for index, want := range wants {
		if events[index].Type != want {
			t.Fatalf("event %d type = %q, want %q", index, events[index].Type, want)
		}
	}
}

func TestSendRemoteRequestDeliversFinalRetryableResponse(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.WriteHeader(http.StatusServiceUnavailable)
		if _, err := writer.Write([]byte("unavailable")); err != nil {
			t.Errorf("write response: %v", err)
		}
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.RetryPolicy = RemoteRetryPolicy{MaxAttempts: 1}
	events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
	assertRemoteEventTypes(t, events, []PrimitiveEventType{
		PrimitiveEventRemoteResponseStarted,
		PrimitiveEventRemoteOutput,
		PrimitiveEventRemoteCompleted,
	})
	started := events[0].Result.(RemoteResponseStartedResult)
	if started.Attempt != 1 || started.StatusCode != http.StatusServiceUnavailable {
		t.Fatalf("response started = %#v", started)
	}
	if output := remoteAttemptOutput(t, events, 1); string(output) != "unavailable" {
		t.Fatalf("output = %q", output)
	}
	if completed := events[len(events)-1].Result.(RemoteCompletedResult); completed.Attempt != 1 {
		t.Fatalf("completed = %#v", completed)
	}
}

func TestSendRemoteRequestRetriesResponseIdleTimeout(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		var attempts atomic.Int32
		client := &RemoteClient{httpClient: &http.Client{Transport: remoteRoundTripFunc(
			func(request *http.Request) (*http.Response, error) {
				attempts.Add(1)
				<-request.Context().Done()
				return nil, request.Context().Err()
			},
		)}}
		request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
		request.ResponseIdleTimeout = time.Second
		request.RetryPolicy = RemoteRetryPolicy{MaxAttempts: 2}
		events := collectInternalEvents(sendRemoteRequest(client, t.Context(), request))
		if len(events) != 2 || events[0].Type != PrimitiveEventRemoteRetryScheduled ||
			events[1].Type != PrimitiveEventFailed {
			t.Fatalf("events = %#v", events)
		}
		if attempts.Load() != 2 {
			t.Fatalf("attempts = %d, want 2", attempts.Load())
		}
		retry := events[0].Result.(RemoteRetryScheduledResult)
		if retry.Attempt != 1 || retry.Delay != 0 || !strings.Contains(retry.Reason, "deadline exceeded") {
			t.Fatalf("retry = %#v", retry)
		}
		result := events[1].Result.(PrimitiveFailureResult)
		if !strings.Contains(result.Error, "attempt 2") ||
			!strings.Contains(result.Error, "deadline exceeded") {
			t.Fatalf("error = %q", result.Error)
		}
	})
}

func TestSendRemoteRequestResponseIdleTimeoutResetsOnBodyProgress(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		body := &delayedRemoteBody{
			delay: 750 * time.Millisecond,
			chunks: [][]byte{
				[]byte("one"),
				[]byte("two"),
				[]byte("three"),
			},
		}
		client := &RemoteClient{httpClient: remoteTestClient(body)}
		request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
		request.ResponseIdleTimeout = time.Second
		request.RetryPolicy.MaxAttempts = 1
		startedAt := time.Now()

		events := collectInternalEvents(sendRemoteRequest(client, t.Context(), request))
		if elapsed := time.Since(startedAt); elapsed <= request.ResponseIdleTimeout {
			t.Fatalf("elapsed = %s, want more than one idle interval", elapsed)
		}
		assertRemoteEventTypes(t, events, []PrimitiveEventType{
			PrimitiveEventRemoteResponseStarted,
			PrimitiveEventRemoteOutput,
			PrimitiveEventRemoteCompleted,
		})
		if output := remoteAttemptOutput(t, events, 1); string(output) != "onetwothree" {
			t.Fatalf("output = %q", output)
		}
	})
}

func TestSendRemoteRequestResponseIdleTimeoutExcludesConsumerBackpressure(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		body := &trackingRemoteBody{Reader: &chunkRemoteReader{
			chunks: [][]byte{nil, []byte("one"), []byte("two")},
		}}
		client := &RemoteClient{httpClient: remoteTestClient(body)}
		request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
		request.ResponseIdleTimeout = time.Second
		request.RetryPolicy.MaxAttempts = 1
		events := sendRemoteRequest(client, t.Context(), request)

		started := <-events
		if started.Type != PrimitiveEventRemoteResponseStarted {
			t.Fatalf("first event = %#v", started)
		}
		synctest.Wait()
		time.Sleep(2 * request.ResponseIdleTimeout)
		rest := collectInternalEvents(events)
		collected := append([]PrimitiveEvent{started}, rest...)
		assertRemoteEventTypes(t, collected, []PrimitiveEventType{
			PrimitiveEventRemoteResponseStarted,
			PrimitiveEventRemoteOutput,
			PrimitiveEventRemoteCompleted,
		})
		if output := remoteAttemptOutput(t, collected, 1); string(output) != "onetwo" {
			t.Fatalf("output = %q", output)
		}
	})
}

func TestSendRemoteRequestIgnoresResponseCloseErrorAfterEOF(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		closeErr := errors.New("close failed")
		body := &trackingRemoteBody{
			Reader:     strings.NewReader("complete"),
			closeDelay: 2 * time.Second,
			closeErr:   closeErr,
		}
		client := &RemoteClient{httpClient: remoteTestClient(body)}
		request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
		request.ResponseIdleTimeout = time.Second
		request.RetryPolicy.MaxAttempts = 1

		events := collectInternalEvents(sendRemoteRequest(client, t.Context(), request))
		assertRemoteEventTypes(t, events, []PrimitiveEventType{
			PrimitiveEventRemoteResponseStarted,
			PrimitiveEventRemoteOutput,
			PrimitiveEventRemoteCompleted,
		})
		if !body.closed {
			t.Fatal("response body was not closed")
		}
	})
}

func TestRemoteErrorRetryClassificationIsConservative(t *testing.T) {
	tests := []struct {
		name      string
		err       error
		retryable bool
	}{
		{name: "idle timeout", err: context.DeadlineExceeded, retryable: true},
		{name: "closed response", err: io.ErrUnexpectedEOF, retryable: true},
		{
			name:      "temporary DNS failure",
			err:       &net.DNSError{Err: "temporary", IsTemporary: true},
			retryable: true,
		},
		{name: "connection reset", err: syscall.ECONNRESET, retryable: true},
		{name: "network timeout", err: remoteTimeoutError{}, retryable: true},
		{name: "caller cancellation", err: context.Canceled},
		{name: "unknown error", err: errors.New("permanent")},
		{name: "untrusted certificate", err: x509.UnknownAuthorityError{}},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			if got := isRetryableRemoteError(test.err); got != test.retryable {
				t.Fatalf("retryable = %t, want %t", got, test.retryable)
			}
		})
	}
}

func TestSendRemoteRequestDoesNotRetryRedirectPolicyFailure(t *testing.T) {
	var attempts atomic.Int32
	var server *httptest.Server
	server = httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		attempts.Add(1)
		http.Redirect(writer, request, server.URL, http.StatusFound)
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.RetryPolicy.InitialBackoff = 0
	request.RetryPolicy.MaxBackoff = 0
	event := singleInternalEvent(
		t,
		collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request)),
		PrimitiveEventFailed,
	)
	if attempts.Load() != 10 {
		t.Fatalf("redirect requests = %d, want 10 from one attempt", attempts.Load())
	}
	failure := event.Result.(PrimitiveFailureResult)
	if !strings.Contains(failure.Error, "stopped after 10 redirects") {
		t.Fatalf("error = %q", failure.Error)
	}
}

func TestSendRemoteRequestCancellationInterruptsOutstandingRequest(t *testing.T) {
	requestStarted := make(chan struct{}, 1)
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		requestStarted <- struct{}{}
		<-request.Context().Done()
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	ctx, cancel := context.WithCancel(t.Context())
	events := sendTestRemoteRequest(t, ctx, request)
	select {
	case <-requestStarted:
	case <-t.Context().Done():
		t.Fatal("request did not start")
	}
	cancel()

	event := singleInternalEvent(t, collectInternalEvents(events), PrimitiveEventCanceled)
	if event.Source != request.Source || event.CorrelationID != request.CorrelationID {
		t.Fatalf("event identity = (%q, %q)", event.Source, event.CorrelationID)
	}
}

func TestSendRemoteRequestCancellationInterruptsRetryBackoff(t *testing.T) {
	var attempts atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		attempts.Add(1)
		writer.WriteHeader(http.StatusServiceUnavailable)
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.RetryPolicy.MaxAttempts = 2
	request.RetryPolicy.InitialBackoff = time.Hour
	request.RetryPolicy.MaxBackoff = time.Hour
	ctx, cancel := context.WithCancel(t.Context())
	events := sendTestRemoteRequest(t, ctx, request)
	started := <-events
	if started.Type != PrimitiveEventRemoteResponseStarted {
		t.Fatalf("first event = %#v", started)
	}
	retryEvent := <-events
	if retryEvent.Type != PrimitiveEventRemoteRetryScheduled {
		t.Fatalf("second event = %#v", retryEvent)
	}
	retry := retryEvent.Result.(RemoteRetryScheduledResult)
	if retry.Attempt != 1 || retry.Delay != time.Hour {
		t.Fatalf("retry = %#v", retry)
	}
	cancel()

	singleInternalEvent(t, collectInternalEvents(events), PrimitiveEventCanceled)
	if attempts.Load() != 1 {
		t.Fatalf("attempts = %d, want 1", attempts.Load())
	}
}

func TestSendRemoteRequestReportsStreamFailure(t *testing.T) {
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		writer.Header().Set("Content-Length", "100")
		writer.WriteHeader(http.StatusOK)
		if _, err := writer.Write([]byte("short")); err != nil {
			t.Errorf("write response: %v", err)
		}
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.RetryPolicy = RemoteRetryPolicy{MaxAttempts: 1}
	events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
	assertRemoteEventTypes(t, events, []PrimitiveEventType{
		PrimitiveEventRemoteResponseStarted,
		PrimitiveEventRemoteOutput,
		PrimitiveEventRemoteStreamFailed,
		PrimitiveEventFailed,
	})
	if output := remoteAttemptOutput(t, events, 1); string(output) != "short" {
		t.Fatalf("output = %q", output)
	}
	failure := remoteEventsOfType(events, PrimitiveEventRemoteStreamFailed)[0].Result.(RemoteStreamFailureResult)
	if failure.Attempt != 1 || !strings.Contains(failure.Error, "unexpected EOF") {
		t.Fatalf("stream failure = %#v", failure)
	}
	terminal := events[len(events)-1].Result.(PrimitiveFailureResult)
	if !strings.Contains(terminal.Error, "unexpected EOF") {
		t.Fatalf("terminal error = %q", terminal.Error)
	}
}

func TestSendRemoteRequestRetriesStreamFailure(t *testing.T) {
	var attempts atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		if attempts.Add(1) == 1 {
			writer.Header().Set("Content-Length", "100")
			writer.WriteHeader(http.StatusOK)
			if _, err := writer.Write([]byte("partial")); err != nil {
				t.Errorf("write partial response: %v", err)
			}
			return
		}
		writer.WriteHeader(http.StatusOK)
		if _, err := writer.Write([]byte("recovered")); err != nil {
			t.Errorf("write recovered response: %v", err)
		}
	}))
	defer server.Close()

	request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
	request.RetryPolicy = RemoteRetryPolicy{MaxAttempts: 2}
	events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
	if attempts.Load() != 2 {
		t.Fatalf("attempts = %d, want 2", attempts.Load())
	}
	assertRemoteEventTypes(t, events, []PrimitiveEventType{
		PrimitiveEventRemoteResponseStarted,
		PrimitiveEventRemoteOutput,
		PrimitiveEventRemoteStreamFailed,
		PrimitiveEventRemoteRetryScheduled,
		PrimitiveEventRemoteResponseStarted,
		PrimitiveEventRemoteOutput,
		PrimitiveEventRemoteCompleted,
	})
	if output := remoteAttemptOutput(t, events, 1); string(output) != "partial" {
		t.Fatalf("partial output = %q", output)
	}
	if failure := remoteEventsOfType(events, PrimitiveEventRemoteStreamFailed)[0].Result.(RemoteStreamFailureResult); failure.Attempt != 1 ||
		!strings.Contains(failure.Error, "unexpected EOF") {
		t.Fatalf("stream failure = %#v", failure)
	}
	if retry := remoteEventsOfType(events, PrimitiveEventRemoteRetryScheduled)[0].Result.(RemoteRetryScheduledResult); retry.Attempt != 1 ||
		!strings.Contains(retry.Reason, "unexpected EOF") {
		t.Fatalf("retry = %#v", retry)
	}
	if output := remoteAttemptOutput(t, events, 2); string(output) != "recovered" {
		t.Fatalf("recovered output = %q", output)
	}
	if completed := events[len(events)-1].Result.(RemoteCompletedResult); completed.Attempt != 2 {
		t.Fatalf("completed = %#v", completed)
	}
}

func TestSendRemoteRequestRejectsInvalidRequest(t *testing.T) {
	valid := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
	tests := []struct {
		name    string
		mutate  func(*RemoteRequest)
		message string
	}{
		{
			name:    "method",
			mutate:  func(request *RemoteRequest) { request.Method = "" },
			message: "method must be set",
		},
		{
			name:    "URL",
			mutate:  func(request *RemoteRequest) { request.URL = "/relative" },
			message: "absolute HTTP or HTTPS URL",
		},
		{
			name:    "malformed HTTP request",
			mutate:  func(request *RemoteRequest) { request.Method = "invalid\nmethod" },
			message: "invalid method",
		},
		{
			name:    "response idle timeout",
			mutate:  func(request *RemoteRequest) { request.ResponseIdleTimeout = 0 },
			message: "response idle timeout must be positive",
		},
		{
			name: "SSE frame size",
			mutate: func(request *RemoteRequest) {
				request.SSE = &RemoteSSEOptions{}
			},
			message: "SSE maximum frame size must be positive",
		},
		{
			name: "SSE frame delimiter",
			mutate: func(request *RemoteRequest) {
				request.SSE = &RemoteSSEOptions{
					MaxFrameSize:   1,
					FrameDelimiter: SSEFrameDelimiter(2),
				}
			},
			message: "invalid SSE frame delimiter mode",
		},
		{
			name:    "attempts",
			mutate:  func(request *RemoteRequest) { request.RetryPolicy.MaxAttempts = 0 },
			message: "maximum attempts must be positive",
		},
		{
			name: "initial backoff",
			mutate: func(request *RemoteRequest) {
				request.RetryPolicy.InitialBackoff = -time.Second
			},
			message: "initial backoff must not be negative",
		},
		{
			name: "maximum backoff",
			mutate: func(request *RemoteRequest) {
				request.RetryPolicy.MaxBackoff = request.RetryPolicy.InitialBackoff - time.Second
			},
			message: "maximum backoff must not be less than initial backoff",
		},
		{
			name: "positive maximum with zero initial backoff",
			mutate: func(request *RemoteRequest) {
				request.RetryPolicy.InitialBackoff = 0
				request.RetryPolicy.MaxBackoff = time.Second
			},
			message: "maximum backoff must be zero when initial backoff is zero",
		},
		{
			name: "retryable status code",
			mutate: func(request *RemoteRequest) {
				request.RetryPolicy.RetryableStatusCodes = []int{99}
			},
			message: "retryable status code 99 must be between 100 and 599",
		},
	}

	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			request := valid
			test.mutate(&request)
			event := singleInternalEvent(
				t,
				collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request)),
				PrimitiveEventFailed,
			)
			result := event.Result.(PrimitiveFailureResult)
			if !strings.Contains(result.Error, test.message) {
				t.Fatalf("error = %q, want %q", result.Error, test.message)
			}
		})
	}
}

func TestSendRemoteRequestRejectsInvalidHeadersBeforeAttempt(t *testing.T) {
	var attempts atomic.Int32
	server := httptest.NewServer(http.HandlerFunc(func(writer http.ResponseWriter, request *http.Request) {
		attempts.Add(1)
		writer.WriteHeader(http.StatusNoContent)
	}))
	defer server.Close()

	tests := []struct {
		name    string
		headers map[string][]string
	}{
		{name: "empty name", headers: map[string][]string{"": {"value"}}},
		{name: "name", headers: map[string][]string{"Invalid\nName": {"value"}}},
		{name: "value", headers: map[string][]string{"Header": {"invalid\rvalue"}}},
	}
	for _, test := range tests {
		t.Run(test.name, func(t *testing.T) {
			request := DefaultRemoteRequest("operation-1", "remote-1", server.URL)
			request.Headers = test.headers
			event := singleInternalEvent(
				t,
				collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request)),
				PrimitiveEventFailed,
			)
			failure := event.Result.(PrimitiveFailureResult)
			if !strings.Contains(failure.Error, "invalid header field") {
				t.Fatalf("error = %q", failure.Error)
			}
		})
	}
	if attempts.Load() != 0 {
		t.Fatalf("attempts = %d, want 0", attempts.Load())
	}
}

func TestPrepareRemoteRequestCanonicalizesHeadersAndForwardsHost(t *testing.T) {
	request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
	request.Headers = map[string][]string{
		"x-request-id": {"one", "two"},
		"host":         {"tenant.example.com"},
	}

	prepared, err := prepareRemoteRequest(request)
	if err != nil {
		t.Fatalf("prepare request: %v", err)
	}
	for name := range prepared.Header {
		if name == "x-request-id" {
			t.Fatalf("headers were not canonicalized: %#v", prepared.Header)
		}
	}
	if values := prepared.Header.Values("X-Request-Id"); !slices.Equal(values, []string{"one", "two"}) {
		t.Fatalf("X-Request-Id = %#v", values)
	}
	if prepared.Host != "tenant.example.com" {
		t.Fatalf("Host = %q", prepared.Host)
	}
	if prepared.Header.Get("Host") != "" {
		t.Fatalf("Host was left in Header: %#v", prepared.Header)
	}
}

func TestPrepareRemoteRequestRejectsMultipleHostValues(t *testing.T) {
	request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
	request.Headers = map[string][]string{"Host": {"one.example.com", "two.example.com"}}

	_, err := prepareRemoteRequest(request)
	if err == nil || !strings.Contains(err.Error(), "Host header must have at most one value") {
		t.Fatalf("error = %v", err)
	}
}

func TestSendRemoteRequestCanceledBeforeStart(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")

	event := singleInternalEvent(
		t,
		collectInternalEvents(sendTestRemoteRequest(t, ctx, request)),
		PrimitiveEventCanceled,
	)
	if event.Source != request.Source || event.CorrelationID != request.CorrelationID {
		t.Fatalf("event identity = (%q, %q)", event.Source, event.CorrelationID)
	}
}

func TestSendRemoteEventCanceledBeforeSend(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	if sendRemoteEvent(ctx, make(chan PrimitiveEvent), PrimitiveEvent{}) {
		t.Fatal("event was sent after cancellation")
	}
}

func TestSendRemoteEventCancellationInterruptsBlockedSend(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		ctx, cancel := context.WithCancel(t.Context())
		result := make(chan bool, 1)
		go func() {
			result <- sendRemoteEvent(ctx, make(chan PrimitiveEvent), PrimitiveEvent{})
		}()
		synctest.Wait()

		cancel()
		if <-result {
			t.Fatal("blocked event was sent after cancellation")
		}
	})
}

func TestStreamRemoteResponseCanceledBeforeStartEvent(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	body := &trackingRemoteBody{Reader: strings.NewReader("unused")}
	response := &http.Response{
		StatusCode: http.StatusOK,
		Header:     make(http.Header),
		Body:       body,
	}
	request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
	_, idleTimeout := newRemoteResponseIdleTimer(ctx, request.ResponseIdleTimeout)
	defer idleTimeout.stop()

	err := streamRemoteResponse(ctx, request, response, make(chan PrimitiveEvent), 1, idleTimeout)
	if !errors.Is(err, context.Canceled) {
		t.Fatalf("error = %v, want context cancellation", err)
	}
	if !body.closed {
		t.Fatal("response body was not closed")
	}
}

func TestStreamRemoteResponseCancellationInterruptsOutput(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		ctx, cancel := context.WithCancel(t.Context())
		events := make(chan PrimitiveEvent, 1)
		body := &trackingRemoteBody{Reader: strings.NewReader("output")}
		response := &http.Response{
			StatusCode: http.StatusOK,
			Header:     make(http.Header),
			Body:       body,
		}
		request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
		_, idleTimeout := newRemoteResponseIdleTimer(ctx, request.ResponseIdleTimeout)
		defer idleTimeout.stop()
		result := make(chan error, 1)
		go func() {
			result <- streamRemoteResponse(ctx, request, response, events, 1, idleTimeout)
		}()
		synctest.Wait()

		cancel()
		if err := <-result; !errors.Is(err, context.Canceled) {
			t.Fatalf("error = %v, want context cancellation", err)
		}
		if !body.closed {
			t.Fatal("response body was not closed")
		}
		if event := <-events; event.Type != PrimitiveEventRemoteResponseStarted {
			t.Fatalf("buffered event = %#v", event)
		}
	})
}

func TestRunRemoteRequestCancellationWinsOverReadAndCloseErrors(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	readErr := errors.New("read failed")
	closeErr := errors.New("close failed")
	client := &http.Client{Transport: remoteRoundTripFunc(func(request *http.Request) (*http.Response, error) {
		return &http.Response{
			StatusCode: http.StatusOK,
			Header:     make(http.Header),
			Body: &cancelingRemoteBody{
				cancel:   cancel,
				err:      readErr,
				closeErr: closeErr,
			},
		}, nil
	})}
	request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
	events := make(chan PrimitiveEvent, 2)

	runRemoteRequest(ctx, client, request, events)
	collected := collectInternalEvents(events)
	if len(collected) != 2 || collected[0].Type != PrimitiveEventRemoteResponseStarted ||
		collected[1].Type != PrimitiveEventCanceled {
		t.Fatalf("events = %#v", collected)
	}
}

func TestRunRemoteRequestCancellationPreservesBufferedResponseEvent(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		ctx, cancel := context.WithCancel(t.Context())
		client := remoteTestClient(&trackingRemoteBody{Reader: strings.NewReader("output")})
		request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
		events := make(chan PrimitiveEvent, 1)
		done := make(chan struct{})
		go func() {
			runRemoteRequest(ctx, client, request, events)
			close(done)
		}()
		synctest.Wait()

		cancel()
		if event := <-events; event.Type != PrimitiveEventRemoteResponseStarted {
			t.Fatalf("buffered event = %#v", event)
		}
		<-done
		if event := <-events; event.Type != PrimitiveEventCanceled {
			t.Fatalf("terminal event = %#v", event)
		}
		assertPrimitiveEventChannelOpen(t, events)
	})
}

func TestRunRemoteRequestCancellationPreservesBufferedOutputEvent(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		ctx, cancel := context.WithCancel(t.Context())
		client := remoteTestClient(&trackingRemoteBody{
			Reader: io.MultiReader(
				strings.NewReader("partial"),
				remoteErrorReader{err: io.ErrUnexpectedEOF},
			),
		})
		request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
		events := make(chan PrimitiveEvent, 1)
		done := make(chan struct{})
		go func() {
			runRemoteRequest(ctx, client, request, events)
			close(done)
		}()

		if event := <-events; event.Type != PrimitiveEventRemoteResponseStarted {
			t.Fatalf("first event = %#v", event)
		}
		synctest.Wait()
		cancel()
		if event := <-events; event.Type != PrimitiveEventRemoteOutput {
			t.Fatalf("buffered event = %#v", event)
		}
		<-done
		if event := <-events; event.Type != PrimitiveEventCanceled {
			t.Fatalf("terminal event = %#v", event)
		}
		assertPrimitiveEventChannelOpen(t, events)
	})
}

func TestRunRemoteRequestCompletedEventDoesNotRequireImmediateDrain(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		client := remoteTestClient(&trackingRemoteBody{Reader: strings.NewReader("")})
		request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
		events := make(chan PrimitiveEvent, 1)
		done := make(chan struct{})
		go func() {
			runRemoteRequest(t.Context(), client, request, events)
			close(done)
		}()

		if event := <-events; event.Type != PrimitiveEventRemoteResponseStarted {
			t.Fatalf("first event = %#v", event)
		}
		synctest.Wait()
		<-done
		if event := <-events; event.Type != PrimitiveEventRemoteCompleted {
			t.Fatalf("terminal event = %#v", event)
		}
		assertPrimitiveEventChannelOpen(t, events)
	})
}

func TestRunRemoteRequestCancellationPreservesQueuedEvent(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		ctx, cancel := context.WithCancel(t.Context())
		client := &http.Client{Transport: remoteRoundTripFunc(func(request *http.Request) (*http.Response, error) {
			return nil, syscall.ECONNRESET
		})}
		request := DefaultRemoteRequest("operation-1", "remote-1", "https://example.com")
		events := make(chan PrimitiveEvent, 1)
		events <- PrimitiveEvent{Type: PrimitiveEventRemoteOutput}
		done := make(chan struct{})
		go func() {
			runRemoteRequest(ctx, client, request, events)
			close(done)
		}()
		synctest.Wait()

		cancel()
		if event := <-events; event.Type != PrimitiveEventRemoteOutput {
			t.Fatalf("queued event = %#v", event)
		}
		<-done
		if event := <-events; event.Type != PrimitiveEventCanceled {
			t.Fatalf("terminal event = %#v", event)
		}
		assertPrimitiveEventChannelOpen(t, events)
	})
}

func TestWaitForRemoteRetryCancellationInterruptsTimer(t *testing.T) {
	synctest.Test(t, func(t *testing.T) {
		ctx, cancel := context.WithCancel(t.Context())
		result := make(chan bool, 1)
		go func() {
			result <- waitForRemoteRetry(ctx, time.Hour)
		}()
		synctest.Wait()

		cancel()
		if <-result {
			t.Fatal("retry wait completed after cancellation")
		}
	})
}

func TestWaitForRemoteRetryCanceledBeforeStart(t *testing.T) {
	ctx, cancel := context.WithCancel(t.Context())
	cancel()
	if waitForRemoteRetry(ctx, time.Hour) {
		t.Fatal("retry wait completed after cancellation")
	}
}

func TestRemoteRetryDelayIsExponentialAndBounded(t *testing.T) {
	policy := RemoteRetryPolicy{
		InitialBackoff: 2 * time.Second,
		MaxBackoff:     5 * time.Second,
	}
	wants := []time.Duration{2 * time.Second, 4 * time.Second, 5 * time.Second, 5 * time.Second}
	for index, want := range wants {
		failedAttempts := index + 1
		if got := policy.Backoff(failedAttempts); got != want {
			t.Fatalf("delay after %d failures = %s, want %s", failedAttempts, got, want)
		}
	}
}

func assertRemoteEventTypes(
	t *testing.T,
	events []PrimitiveEvent,
	want []PrimitiveEventType,
) {
	t.Helper()
	got := make([]PrimitiveEventType, 0, len(events))
	for _, event := range events {
		if event.Type == PrimitiveEventRemoteOutput && len(got) > 0 &&
			got[len(got)-1] == PrimitiveEventRemoteOutput {
			continue
		}
		got = append(got, event.Type)
	}
	if !slices.Equal(got, want) {
		t.Fatalf("event types = %#v, want %#v; events = %#v", got, want, events)
	}
}

func remoteEventsOfType(events []PrimitiveEvent, eventType PrimitiveEventType) []PrimitiveEvent {
	var matching []PrimitiveEvent
	for _, event := range events {
		if event.Type == eventType {
			matching = append(matching, event)
		}
	}
	return matching
}

func remoteAttemptOutput(t *testing.T, events []PrimitiveEvent, attempt int) []byte {
	t.Helper()
	var output []byte
	var offset int64
	for _, event := range events {
		if event.Type != PrimitiveEventRemoteOutput {
			continue
		}
		result := event.Result.(RemoteOutputResult)
		if result.Attempt != attempt {
			continue
		}
		if result.Offset != offset {
			t.Fatalf("attempt %d output offset = %d, want %d", attempt, result.Offset, offset)
		}
		output = append(output, result.Data...)
		offset += int64(len(result.Data))
	}
	return output
}

func assertRemoteEventIdentity(t *testing.T, events []PrimitiveEvent, request RemoteRequest) {
	t.Helper()
	for index, event := range events {
		if event.Source != request.Source || event.CorrelationID != request.CorrelationID {
			t.Fatalf(
				"event %d identity = (%q, %q), want (%q, %q)",
				index,
				event.Source,
				event.CorrelationID,
				request.Source,
				request.CorrelationID,
			)
		}
	}
}

func sendTestRemoteRequest(
	t *testing.T,
	ctx context.Context,
	request RemoteRequest,
) <-chan PrimitiveEvent {
	t.Helper()
	client := NewRemoteClient()
	t.Cleanup(func() {
		if err := client.Close(); err != nil {
			t.Errorf("close remote client: %v", err)
		}
	})
	return sendRemoteRequest(client, ctx, request)
}

func sendRemoteRequest(
	client *RemoteClient,
	ctx context.Context,
	request RemoteRequest,
) <-chan PrimitiveEvent {
	events := make(chan PrimitiveEvent)
	client.SendRequest(ctx, request, events)
	return events
}

type remoteTimeoutError struct{}

func (remoteTimeoutError) Error() string {
	return "network timeout"
}

func (remoteTimeoutError) Timeout() bool {
	return true
}

func (remoteTimeoutError) Temporary() bool {
	return false
}

type trackingRemoteBody struct {
	io.Reader
	closed     bool
	closeDelay time.Duration
	closeErr   error
}

func (body *trackingRemoteBody) Close() error {
	body.closed = true
	time.Sleep(body.closeDelay)
	return body.closeErr
}

type cancelingRemoteBody struct {
	cancel   context.CancelFunc
	err      error
	closeErr error
}

func (body *cancelingRemoteBody) Read(buffer []byte) (int, error) {
	body.cancel()
	return 0, body.err
}

func (body *cancelingRemoteBody) Close() error {
	return body.closeErr
}

type chunkRemoteReader struct {
	chunks [][]byte
	index  int
}

func (reader *chunkRemoteReader) Read(buffer []byte) (int, error) {
	if reader.index == len(reader.chunks) {
		return 0, io.EOF
	}
	count := copy(buffer, reader.chunks[reader.index])
	if count == len(reader.chunks[reader.index]) {
		reader.index++
	} else {
		reader.chunks[reader.index] = reader.chunks[reader.index][count:]
	}
	return count, nil
}

type delayedRemoteBody struct {
	delay  time.Duration
	chunks [][]byte
	index  int
}

func (body *delayedRemoteBody) Read(buffer []byte) (int, error) {
	if body.index == len(body.chunks) {
		return 0, io.EOF
	}
	time.Sleep(body.delay)
	count := copy(buffer, body.chunks[body.index])
	body.index++
	return count, nil
}

func (body *delayedRemoteBody) Close() error {
	return nil
}

type remoteErrorReader struct {
	err error
}

func (reader remoteErrorReader) Read(buffer []byte) (int, error) {
	return 0, reader.err
}

type terminalRemoteReader struct {
	data []byte
	err  error
}

func (reader *terminalRemoteReader) Read(buffer []byte) (int, error) {
	count := copy(buffer, reader.data)
	reader.data = reader.data[count:]
	if len(reader.data) == 0 {
		return count, reader.err
	}
	return count, nil
}

type remoteRoundTripFunc func(*http.Request) (*http.Response, error)

func (roundTrip remoteRoundTripFunc) RoundTrip(request *http.Request) (*http.Response, error) {
	return roundTrip(request)
}

type closeTrackingRemoteTransport struct {
	closeCalls atomic.Int32
}

func (*closeTrackingRemoteTransport) RoundTrip(*http.Request) (*http.Response, error) {
	return nil, errors.New("unexpected request")
}

func (transport *closeTrackingRemoteTransport) CloseIdleConnections() {
	transport.closeCalls.Add(1)
}

func remoteTestClient(body io.ReadCloser) *http.Client {
	return &http.Client{Transport: remoteRoundTripFunc(func(request *http.Request) (*http.Response, error) {
		return &http.Response{
			StatusCode: http.StatusOK,
			Header:     make(http.Header),
			Body:       body,
		}, nil
	})}
}

func TestRemoteSSEContentTypeSelection(t *testing.T) {
	for _, test := range []struct {
		name, contentType  string
		status             int
		requestSSE, framed bool
	}{
		{name: "missing", requestSSE: true, framed: true},
		{name: "missing 299", status: 299, requestSSE: true, framed: true},
		{name: "missing 300", status: 300, requestSSE: true},
		{name: "missing 401", status: 401, requestSSE: true},
		{name: "missing 503", status: 503, requestSSE: true},
		{name: "SSE", contentType: "text/event-stream", requestSSE: true, framed: true},
		{name: "SSE parameters", contentType: "text/event-stream; charset=utf-8", requestSSE: true, framed: true},
		{name: "SSE case", contentType: "Text/Event-Stream", requestSSE: true, framed: true},
		{name: "JSON", contentType: "application/json", requestSSE: true},
		{name: "text", contentType: "text/plain", requestSSE: true},
		{name: "malformed", contentType: "text/event-stream; broken", requestSSE: true},
		{name: "missing without opt-in"},
		{name: "SSE without opt-in", contentType: "text/event-stream"},
	} {
		t.Run(test.name, func(t *testing.T) {
			const body = "data: first\r\n\r\ndata: second\n\nunfinished"
			server := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, _ *http.Request) {
				if test.contentType == "" {
					w.Header()["Content-Type"] = nil // Disable net/http's content sniffing.
				} else {
					w.Header().Set("Content-Type", test.contentType)
				}
				if test.status != 0 {
					w.WriteHeader(test.status)
				}
				_, _ = io.WriteString(w, body)
			}))
			defer server.Close()
			request := DefaultRemoteRequest("test", "content-type", server.URL)
			request.RetryPolicy.MaxAttempts = 1
			if test.requestSSE {
				request.SSE = &RemoteSSEOptions{MaxFrameSize: 1024, FrameDelimiter: SSEFrameDelimiterStrip}
			}
			events := collectInternalEvents(sendTestRemoteRequest(t, t.Context(), request))
			started := remoteEventsOfType(events, PrimitiveEventRemoteResponseStarted)[0].Result.(RemoteResponseStartedResult)
			if http.Header(started.Headers).Get("Content-Type") != test.contentType {
				t.Fatal("unexpected test response content type")
			}
			if test.framed {
				outputs := remoteEventsOfType(events, PrimitiveEventRemoteOutput)
				if len(outputs) != 2 {
					t.Fatalf("frames = %d", len(outputs))
				}
				for i, want := range []string{"data: first", "data: second"} {
					if got := string(outputs[i].Result.(RemoteOutputResult).Data); got != want {
						t.Fatalf("frame %d = %q, want %q", i, got, want)
					}
				}
			} else if got := string(remoteAttemptOutput(t, events, 1)); got != body {
				t.Fatalf("raw body = %q", got)
			}
			if events[len(events)-1].Type != PrimitiveEventRemoteCompleted {
				t.Fatal("request did not complete")
			}
		})
	}
}
