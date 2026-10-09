package primitives

import (
	"bytes"
	"context"
	"errors"
	"fmt"
	"io"
	"mime"
	"net"
	"net/http"
	"slices"
	"strings"
	"syscall"
	"time"
)

const (
	PrimitiveEventRemoteResponseStarted PrimitiveEventType = "remote.response_started"
	PrimitiveEventRemoteOutput          PrimitiveEventType = "remote.output"
	PrimitiveEventRemoteStreamFailed    PrimitiveEventType = "remote.stream_failed"
	PrimitiveEventRemoteRetryScheduled  PrimitiveEventType = "remote.retry_scheduled"
	PrimitiveEventRemoteCompleted       PrimitiveEventType = "remote.completed"
)

const DefaultRemoteMaxAttempts = 5

type RemoteRequest struct {
	Source              SourceID
	CorrelationID       CorrelationID
	Method              string
	URL                 string
	Headers             map[string][]string
	Body                []byte
	ResponseIdleTimeout time.Duration
	SSE                 *RemoteSSEOptions
	RetryPolicy         RemoteRetryPolicy
}

type RemoteSSEOptions struct {
	MaxFrameSize   int64
	FrameDelimiter SSEFrameDelimiter
}

type SSEFrameDelimiter uint8

const (
	SSEFrameDelimiterStrip SSEFrameDelimiter = iota
	SSEFrameDelimiterPassThrough
)

func DefaultRemoteRequest(source SourceID, correlationID CorrelationID, url string) RemoteRequest {
	return RemoteRequest{
		Source:              source,
		CorrelationID:       correlationID,
		Method:              http.MethodGet,
		URL:                 url,
		Headers:             make(map[string][]string),
		ResponseIdleTimeout: 30 * time.Second,
		RetryPolicy: RemoteRetryPolicy{
			MaxAttempts:    DefaultRemoteMaxAttempts,
			InitialBackoff: 2 * time.Second,
			MaxBackoff:     30 * time.Second,
			RetryableStatusCodes: []int{
				http.StatusRequestTimeout,
				http.StatusTooEarly,
				http.StatusTooManyRequests,
				http.StatusInternalServerError,
				http.StatusBadGateway,
				http.StatusServiceUnavailable,
				http.StatusGatewayTimeout,
				520,
				521,
				522,
				523,
				524,
				529,
			},
		},
	}
}

type RemoteRetryPolicy struct {
	MaxAttempts          int
	InitialBackoff       time.Duration
	MaxBackoff           time.Duration
	RetryableStatusCodes []int
}

type RemoteResponseStartedResult struct {
	Attempt    int
	StatusCode int
	Headers    map[string][]string
}

type RemoteOutputResult struct {
	Attempt int
	Offset  int64
	Data    []byte
}

type RemoteStreamFailureResult struct {
	Attempt int
	Error   string
}

type RemoteRetryScheduledResult struct {
	Attempt int
	Reason  string
	Delay   time.Duration
}

type RemoteCompletedResult struct {
	Attempt int
}

type RemoteClient struct {
	httpClient *http.Client
}

func NewRemoteClient() *RemoteClient {
	return &RemoteClient{httpClient: newRemoteHTTPClient()}
}

func NewRemoteClientWithHTTPClient(httpClient *http.Client) *RemoteClient {
	return &RemoteClient{httpClient: httpClient}
}

func (client *RemoteClient) Close() error {
	client.httpClient.CloseIdleConnections()
	return nil
}

func (client *RemoteClient) SendRequest(
	ctx context.Context,
	request RemoteRequest,
	events chan<- PrimitiveEvent,
) {
	go runRemoteRequest(ctx, client.httpClient, request, events)
}

func newRemoteHTTPClient() *http.Client {
	dialer := &net.Dialer{
		Timeout:   30 * time.Second,
		KeepAlive: 30 * time.Second,
	}
	transport := &http.Transport{
		Proxy:                 http.ProxyFromEnvironment,
		DialContext:           dialer.DialContext,
		ForceAttemptHTTP2:     true,
		MaxIdleConns:          100,
		IdleConnTimeout:       90 * time.Second,
		TLSHandshakeTimeout:   10 * time.Second,
		ExpectContinueTimeout: time.Second,
	}
	return &http.Client{Transport: transport}
}

func runRemoteRequest(
	ctx context.Context,
	client *http.Client,
	request RemoteRequest,
	events chan<- PrimitiveEvent,
) {
	baseRequest, err := prepareRemoteRequest(request)
	if err != nil {
		sendRemoteTerminalEvent(events, remoteFailure(request, err))
		return
	}
	if ctx.Err() != nil {
		sendRemoteTerminalEvent(events, remoteCanceled(request))
		return
	}

	for attempt := 1; attempt <= request.RetryPolicy.MaxAttempts; attempt++ {
		attemptContext, idleTimeout := newRemoteResponseIdleTimer(
			ctx,
			request.ResponseIdleTimeout,
		)
		httpRequest := baseRequest.Clone(attemptContext)
		if len(request.Body) == 0 {
			httpRequest.Body = http.NoBody
		} else {
			httpRequest.ContentLength = int64(len(request.Body))
			httpRequest.Body = newRemoteRequestBody(request.Body)
			httpRequest.GetBody = func() (io.ReadCloser, error) {
				return newRemoteRequestBody(request.Body), nil
			}
		}

		response, attemptErr := client.Do(httpRequest)
		streamFailed := false
		if attemptErr == nil {
			idleTimeout.pause()
			attemptErr = streamRemoteResponse(
				ctx,
				request,
				response,
				events,
				attempt,
				idleTimeout,
			)
			streamFailed = attemptErr != nil
		} else if response != nil && response.Body != nil {
			attemptErr = errors.Join(attemptErr, response.Body.Close())
		}
		attemptErr = normalizeRemoteAttemptError(ctx, attemptContext, attemptErr)
		idleTimeout.stop()

		if ctx.Err() != nil {
			sendRemoteTerminalEvent(events, remoteCanceled(request))
			return
		}
		if streamFailed && !sendRemoteEvent(
			ctx,
			events,
			remoteStreamFailure(request, attempt, attemptErr),
		) {
			sendRemoteTerminalEvent(events, remoteCanceled(request))
			return
		}
		if attemptErr != nil && !isRetryableRemoteError(attemptErr) {
			sendRemoteTerminalEvent(
				events,
				remoteFailure(request, remoteAttemptFailure(request, attempt, attemptErr)),
			)
			return
		}

		if attemptErr == nil {
			if !isRetryableRemoteStatus(request.RetryPolicy, response.StatusCode) ||
				attempt == request.RetryPolicy.MaxAttempts {
				sendRemoteTerminalEvent(events, PrimitiveEvent{
					Type:          PrimitiveEventRemoteCompleted,
					Source:        request.Source,
					CorrelationID: request.CorrelationID,
					Result:        RemoteCompletedResult{Attempt: attempt},
				})
				return
			}
			attemptErr = fmt.Errorf(
				"response status %d %s is retryable",
				response.StatusCode,
				http.StatusText(response.StatusCode),
			)
		}

		if attempt == request.RetryPolicy.MaxAttempts {
			sendRemoteTerminalEvent(
				events,
				remoteFailure(request, remoteAttemptFailure(request, attempt, attemptErr)),
			)
			return
		}
		delay := request.RetryPolicy.Backoff(attempt)
		if !sendRemoteEvent(ctx, events, remoteRetryScheduled(request, attempt, attemptErr, delay)) {
			sendRemoteTerminalEvent(events, remoteCanceled(request))
			return
		}
		if !waitForRemoteRetry(ctx, delay) {
			sendRemoteTerminalEvent(events, remoteCanceled(request))
			return
		}
	}
}

func streamRemoteResponse(
	ctx context.Context,
	request RemoteRequest,
	response *http.Response,
	events chan<- PrimitiveEvent,
	attempt int,
	idleTimeout *remoteResponseIdleTimer,
) error {
	started := PrimitiveEvent{
		Type:          PrimitiveEventRemoteResponseStarted,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result: RemoteResponseStartedResult{
			Attempt:    attempt,
			StatusCode: response.StatusCode,
			Headers:    response.Header.Clone(),
		},
	}
	if !sendRemoteEvent(ctx, events, started) {
		_ = response.Body.Close()
		return ctx.Err()
	}

	frameSSE := false
	if request.SSE != nil {
		contentType := response.Header.Get("Content-Type")
		mediaType, _, err := mime.ParseMediaType(contentType)
		frameSSE = err == nil && mediaType == "text/event-stream"
		if contentType == "" {
			frameSSE = response.StatusCode >= http.StatusOK && response.StatusCode < http.StatusMultipleChoices
		}
	}
	var sseFramer *remoteSSEFramer
	if frameSSE {
		sseFramer = newRemoteSSEFramer(request.SSE.MaxFrameSize, request.SSE.FrameDelimiter)
	}
	buffer := make([]byte, IOReadChunkSize)
	var offset int64
	idleTimeout.arm()
	for {
		count, readErr := response.Body.Read(buffer)
		if count <= 0 && readErr == nil {
			continue
		}
		idleTimeout.pause()
		if count > 0 {
			if frameSSE {
				frames, frameErr := sseFramer.append(buffer[:count])
				for _, frame := range frames {
					if !sendRemoteOutput(ctx, request, events, attempt, frame.offset, frame.data) {
						_ = response.Body.Close()
						return ctx.Err()
					}
				}
				if frameErr != nil {
					return errors.Join(frameErr, response.Body.Close())
				}
			} else {
				if !sendRemoteOutput(
					ctx,
					request,
					events,
					attempt,
					offset,
					append([]byte(nil), buffer[:count]...),
				) {
					_ = response.Body.Close()
					return ctx.Err()
				}
				offset += int64(count)
			}
		}

		if readErr != nil {
			if frameSSE {
				for _, frame := range sseFramer.finish() {
					if !sendRemoteOutput(ctx, request, events, attempt, frame.offset, frame.data) {
						_ = response.Body.Close()
						return ctx.Err()
					}
				}
			}
			if errors.Is(readErr, io.EOF) {
				_ = response.Body.Close()
				return nil
			}
			return errors.Join(readErr, response.Body.Close())
		}
		idleTimeout.arm()
	}
}

func sendRemoteOutput(
	ctx context.Context,
	request RemoteRequest,
	events chan<- PrimitiveEvent,
	attempt int,
	offset int64,
	data []byte,
) bool {
	return sendRemoteEvent(ctx, events, PrimitiveEvent{
		Type:          PrimitiveEventRemoteOutput,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result: RemoteOutputResult{
			Attempt: attempt,
			Offset:  offset,
			Data:    data,
		},
	})
}

func newRemoteRequestBody(body []byte) io.ReadCloser {
	return io.NopCloser(bytes.NewReader(body))
}

type remoteResponseIdleTimer struct {
	timeout time.Duration
	cancel  context.CancelCauseFunc
	timer   *time.Timer
}

func newRemoteResponseIdleTimer(
	parent context.Context,
	timeout time.Duration,
) (context.Context, *remoteResponseIdleTimer) {
	ctx, cancel := context.WithCancelCause(parent)
	idleTimeout := &remoteResponseIdleTimer{
		timeout: timeout,
		cancel:  cancel,
	}
	idleTimeout.timer = time.AfterFunc(timeout, func() {
		cancel(context.DeadlineExceeded)
	})
	return ctx, idleTimeout
}

func (idleTimeout *remoteResponseIdleTimer) arm() {
	idleTimeout.timer.Reset(idleTimeout.timeout)
}

func (idleTimeout *remoteResponseIdleTimer) pause() {
	idleTimeout.timer.Stop()
}

func (idleTimeout *remoteResponseIdleTimer) stop() {
	idleTimeout.timer.Stop()
	idleTimeout.cancel(context.Canceled)
}

func normalizeRemoteAttemptError(
	parent context.Context,
	attemptContext context.Context,
	err error,
) error {
	if parent.Err() == nil && errors.Is(context.Cause(attemptContext), context.DeadlineExceeded) {
		return fmt.Errorf("remote request response idle timeout: %w", context.DeadlineExceeded)
	}
	return err
}

func remoteStreamFailure(
	request RemoteRequest,
	attempt int,
	err error,
) PrimitiveEvent {
	return PrimitiveEvent{
		Type:          PrimitiveEventRemoteStreamFailed,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result: RemoteStreamFailureResult{
			Attempt: attempt,
			Error:   err.Error(),
		},
	}
}

func sendRemoteEvent(
	ctx context.Context,
	events chan<- PrimitiveEvent,
	event PrimitiveEvent,
) bool {
	if ctx.Err() != nil {
		return false
	}
	select {
	case events <- event:
		return true
	case <-ctx.Done():
		return false
	}
}

func sendRemoteTerminalEvent(events chan<- PrimitiveEvent, event PrimitiveEvent) {
	events <- event
}

func prepareRemoteRequest(request RemoteRequest) (*http.Request, error) {
	if request.Method == "" {
		return nil, errors.New("send remote request: method must be set")
	}
	parsed, err := http.NewRequest(request.Method, request.URL, nil)
	if err != nil {
		return nil, fmt.Errorf("send remote request: %w", err)
	}
	if parsed.URL.Host == "" || (parsed.URL.Scheme != "http" && parsed.URL.Scheme != "https") {
		return nil, errors.New("send remote request: URL must be an absolute HTTP or HTTPS URL")
	}
	if request.ResponseIdleTimeout <= 0 {
		return nil, errors.New("send remote request: response idle timeout must be positive")
	}
	if request.SSE != nil {
		if request.SSE.MaxFrameSize <= 0 {
			return nil, errors.New("send remote request: SSE maximum frame size must be positive")
		}
		if request.SSE.FrameDelimiter != SSEFrameDelimiterStrip &&
			request.SSE.FrameDelimiter != SSEFrameDelimiterPassThrough {
			return nil, errors.New("send remote request: invalid SSE frame delimiter mode")
		}
	}
	if request.RetryPolicy.MaxAttempts <= 0 {
		return nil, errors.New("send remote request: maximum attempts must be positive")
	}
	if request.RetryPolicy.InitialBackoff < 0 {
		return nil, errors.New("send remote request: initial backoff must not be negative")
	}
	if request.RetryPolicy.MaxBackoff < request.RetryPolicy.InitialBackoff {
		return nil, errors.New("send remote request: maximum backoff must not be less than initial backoff")
	}
	if request.RetryPolicy.InitialBackoff == 0 && request.RetryPolicy.MaxBackoff != 0 {
		return nil, errors.New("send remote request: maximum backoff must be zero when initial backoff is zero")
	}
	for _, statusCode := range request.RetryPolicy.RetryableStatusCodes {
		if statusCode < 100 || statusCode > 599 {
			return nil, fmt.Errorf(
				"send remote request: retryable status code %d must be between 100 and 599",
				statusCode,
			)
		}
	}

	var hostValues []string
	for name, values := range request.Headers {
		if strings.EqualFold(name, "Host") {
			hostValues = append(hostValues, values...)
			continue
		}
		for _, value := range values {
			parsed.Header.Add(name, value)
		}
	}
	if len(hostValues) > 1 {
		return nil, errors.New("send remote request: Host header must have at most one value")
	}
	if len(hostValues) == 1 {
		parsed.Host = hostValues[0]
	}
	return parsed, nil
}

func isRetryableRemoteStatus(policy RemoteRetryPolicy, statusCode int) bool {
	return slices.Contains(policy.RetryableStatusCodes, statusCode)
}

func isRetryableRemoteError(err error) bool {
	if errors.Is(err, context.DeadlineExceeded) ||
		errors.Is(err, io.EOF) ||
		errors.Is(err, io.ErrUnexpectedEOF) {
		return true
	}

	var networkError net.Error
	if errors.As(err, &networkError) && networkError.Timeout() {
		return true
	}
	var temporaryError interface{ Temporary() bool }
	if errors.As(err, &temporaryError) && temporaryError.Temporary() {
		return true
	}

	return errors.Is(err, syscall.ECONNABORTED) ||
		errors.Is(err, syscall.ECONNREFUSED) ||
		errors.Is(err, syscall.ECONNRESET) ||
		errors.Is(err, syscall.EHOSTDOWN) ||
		errors.Is(err, syscall.EHOSTUNREACH) ||
		errors.Is(err, syscall.ENETDOWN) ||
		errors.Is(err, syscall.ENETUNREACH) ||
		errors.Is(err, syscall.EPIPE) ||
		errors.Is(err, syscall.ETIMEDOUT)
}

func (policy RemoteRetryPolicy) Backoff(failedAttempts int) time.Duration {
	delay := policy.InitialBackoff
	for range failedAttempts - 1 {
		if delay >= policy.MaxBackoff || delay > policy.MaxBackoff-delay {
			return policy.MaxBackoff
		}
		delay *= 2
	}
	return min(delay, policy.MaxBackoff)
}

func waitForRemoteRetry(ctx context.Context, delay time.Duration) bool {
	if ctx.Err() != nil {
		return false
	}
	timer := time.NewTimer(delay)
	defer timer.Stop()
	select {
	case <-timer.C:
		return true
	case <-ctx.Done():
		return false
	}
}

func remoteRetryScheduled(
	request RemoteRequest,
	attempt int,
	err error,
	delay time.Duration,
) PrimitiveEvent {
	return PrimitiveEvent{
		Type:          PrimitiveEventRemoteRetryScheduled,
		Source:        request.Source,
		CorrelationID: request.CorrelationID,
		Result: RemoteRetryScheduledResult{
			Attempt: attempt,
			Reason:  err.Error(),
			Delay:   delay,
		},
	}
}

func remoteAttemptFailure(request RemoteRequest, attempt int, err error) error {
	return fmt.Errorf(
		"send remote request %s %q: attempt %d: %w",
		request.Method,
		request.URL,
		attempt,
		err,
	)
}

func remoteFailure(request RemoteRequest, err error) PrimitiveEvent {
	return primitiveFailure(request.Source, request.CorrelationID, err)
}

func remoteCanceled(request RemoteRequest) PrimitiveEvent {
	return primitiveCanceled(request.Source, request.CorrelationID)
}
