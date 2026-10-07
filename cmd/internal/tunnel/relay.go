package tunnel

import (
	"context"
	"crypto/subtle"
	"encoding/json/v2"
	"errors"
	"net"
	"net/http"
	"net/http/httputil"
	"net/url"
	"strconv"
	"strings"
	"sync"
	"time"

	"github.com/gorilla/websocket"
	"github.com/hashicorp/yamux"
)

// Health is what the relay says of itself at HealthPath. To a request
// without the token it says what it is and nothing more.
type Health struct {
	Relay    string `json:"relay"`
	Protocol int    `json:"protocol"`
	// The rest is said to the token only.
	Authorized bool   `json:"authorized"`
	Version    string `json:"version,omitempty"`
	// Agent says that a cockpit is connected, since AgentSince.
	Agent      bool      `json:"agent"`
	AgentSince time.Time `json:"agent_since,omitzero"`
}

// errOffline is the relay's answer while no cockpit is connected.
var errOffline = errors.New("the cockpit is not connected to the relay")

// Relay is the relay's handler: the cockpit connects at AgentPath, and
// every other request, once its token is checked, goes down that
// connection to the cockpit.
type Relay struct {
	token   []byte
	version string
	logf    func(format string, args ...any)

	upgrader  websocket.Upgrader
	transport *http.Transport
	proxy     *httputil.ReverseProxy

	mu    sync.Mutex
	agent *yamux.Session
	since time.Time
}

// NewRelay makes the relay of token; logf, when set, hears the cockpit
// come and go.
func NewRelay(token, version string, logf func(format string, args ...any)) *Relay {
	if logf == nil {
		logf = func(string, ...any) {}
	}
	rl := &Relay{token: []byte(token), version: version, logf: logf}
	rl.upgrader = websocket.Upgrader{
		ReadBufferSize: 32 << 10, WriteBufferSize: 32 << 10,
		// The cockpit is a program, which says no Origin; a page has no
		// business being the cockpit.
		CheckOrigin: func(r *http.Request) bool { return r.Header.Get("Origin") == "" },
	}
	rl.transport = &http.Transport{
		// Each connection to the cockpit is a stream of the session; the
		// address is the session's, whatever the request names.
		DialContext: func(ctx context.Context, _, _ string) (net.Conn, error) {
			session := rl.current()
			if session == nil {
				return nil, errOffline
			}
			return session.Open()
		},
		MaxIdleConns:        64,
		MaxIdleConnsPerHost: 64,
		IdleConnTimeout:     90 * time.Second,
		DisableCompression:  true,
	}
	cockpit := &url.URL{Scheme: "http", Host: "cockpit"}
	rl.proxy = &httputil.ReverseProxy{
		Rewrite: func(pr *httputil.ProxyRequest) {
			pr.SetURL(cockpit)
			// The cockpit sees the host its client asked for: its pages
			// and sockets name it.
			pr.Out.Host = pr.In.Host
			pr.SetXForwarded()
			// The relay's token is the relay's.
			pr.Out.Header.Del("Authorization")
			withoutCookie(pr.Out.Header, TokenCookie)
		},
		Transport:     rl.transport,
		FlushInterval: -1, // events and terminals go as they come
		ErrorHandler: func(w http.ResponseWriter, r *http.Request, err error) {
			if errors.Is(err, errOffline) {
				writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": errOffline.Error(), "code": "agent_offline"})
				return
			}
			if errors.Is(err, context.Canceled) {
				return // the client left
			}
			writeJSON(w, http.StatusBadGateway, map[string]string{"error": "the cockpit did not answer: " + err.Error(), "code": "agent_error"})
		},
	}
	return rl
}

func (rl *Relay) ServeHTTP(w http.ResponseWriter, r *http.Request) {
	switch r.URL.Path {
	case HealthPath:
		rl.serveHealth(w, r)
		return
	case AgentPath:
		rl.serveAgent(w, r)
		return
	}
	if !rl.admit(w, r) {
		return
	}
	if rl.current() == nil {
		writeJSON(w, http.StatusServiceUnavailable, map[string]string{"error": errOffline.Error(), "code": "agent_offline"})
		return
	}
	rl.proxy.ServeHTTP(w, r)
}

// Connected says whether a cockpit is connected, and since when.
func (rl *Relay) Connected() (bool, time.Time) {
	rl.mu.Lock()
	defer rl.mu.Unlock()
	return rl.agent != nil, rl.since
}

// Close lets the cockpit go.
func (rl *Relay) Close() {
	rl.mu.Lock()
	session := rl.agent
	rl.agent = nil
	rl.mu.Unlock()
	if session != nil {
		session.Close()
	}
	rl.transport.CloseIdleConnections()
}

func (rl *Relay) current() *yamux.Session {
	rl.mu.Lock()
	defer rl.mu.Unlock()
	if rl.agent != nil && rl.agent.IsClosed() {
		return nil
	}
	return rl.agent
}

func (rl *Relay) valid(token string) bool {
	return token != "" && subtle.ConstantTimeCompare([]byte(token), rl.token) == 1
}

// authorized says whether the request has the token: in its Authorization
// header, or the cookie of a browser that brought it once.
func (rl *Relay) authorized(r *http.Request) bool {
	if rl.valid(bearer(r)) {
		return true
	}
	cookie, err := r.Cookie(TokenCookie)
	return err == nil && rl.valid(cookie.Value)
}

// admit lets a client's request through, or answers it: without the token
// it is refused; a page opened with the token in its address has it kept in
// a cookie, and goes on to the address without it.
func (rl *Relay) admit(w http.ResponseWriter, r *http.Request) bool {
	query := r.URL.Query()
	if token := query.Get(TokenQuery); token != "" && (r.Method == http.MethodGet || r.Method == http.MethodHead) {
		if !rl.valid(token) {
			writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "that token is not the relay's", "code": "unauthorized"})
			return false
		}
		http.SetCookie(w, &http.Cookie{
			Name: TokenCookie, Value: token, Path: "/", MaxAge: 400 * 24 * 60 * 60,
			HttpOnly: true, Secure: r.TLS != nil, SameSite: http.SameSiteStrictMode,
		})
		query.Del(TokenQuery)
		target := *r.URL
		target.RawQuery = query.Encode()
		w.Header().Set("Cache-Control", "no-store")
		http.Redirect(w, r, target.RequestURI(), http.StatusSeeOther)
		return false
	}
	if rl.authorized(r) {
		return true
	}
	w.Header().Set("WWW-Authenticate", `Bearer realm="`+Name+`"`)
	writeJSON(w, http.StatusUnauthorized, map[string]string{
		"error": "the relay wants its token: Authorization: Bearer <token>, or, in a browser, open /?" + TokenQuery + "=<token> once",
		"code":  "unauthorized",
	})
	return false
}

func (rl *Relay) serveHealth(w http.ResponseWriter, r *http.Request) {
	health := Health{Relay: Name, Protocol: Protocol}
	if rl.authorized(r) {
		health.Authorized, health.Version = true, rl.version
		health.Agent, health.AgentSince = rl.Connected()
		if !health.Agent {
			health.AgentSince = time.Time{}
		}
	}
	w.Header().Set("Cache-Control", "no-store")
	writeJSON(w, http.StatusOK, health)
}

// serveAgent takes the cockpit's connection: a WebSocket, which carries a
// yamux session whose streams the relay opens, one for each connection of
// a client. A cockpit that connects again takes the place of the last.
func (rl *Relay) serveAgent(w http.ResponseWriter, r *http.Request) {
	if !rl.valid(bearer(r)) {
		writeJSON(w, http.StatusUnauthorized, map[string]string{"error": "that token is not the relay's", "code": "unauthorized"})
		return
	}
	if got := r.Header.Get(ProtocolHeader); got != strconv.Itoa(Protocol) {
		writeJSON(w, http.StatusConflict, map[string]string{
			"error": "the relay speaks protocol " + strconv.Itoa(Protocol) + ", the cockpit " + cmp(got, "none") + ": update the older of the two",
			"code":  "incompatible",
		})
		return
	}
	if !websocket.IsWebSocketUpgrade(r) {
		writeJSON(w, http.StatusBadRequest, map[string]string{"error": "the cockpit connects with a WebSocket"})
		return
	}
	ws, err := rl.upgrader.Upgrade(w, r, nil)
	if err != nil {
		return // the upgrader answered
	}
	session, err := yamux.Client(newConn(ws), sessionConfig())
	if err != nil {
		ws.Close()
		return
	}
	rl.mu.Lock()
	previous := rl.agent
	rl.agent, rl.since = session, time.Now()
	rl.mu.Unlock()
	if previous != nil {
		previous.Close()
	}
	// The connections kept for the last cockpit lead nowhere now.
	rl.transport.CloseIdleConnections()
	rl.logf("cockpit connected from %s", clientAddress(r))
	go func() {
		<-session.CloseChan()
		rl.mu.Lock()
		gone := rl.agent == session
		if gone {
			rl.agent = nil
		}
		rl.mu.Unlock()
		if gone {
			rl.transport.CloseIdleConnections()
			rl.logf("cockpit disconnected")
		}
	}()
}

func bearer(r *http.Request) string {
	scheme, token, ok := strings.Cut(r.Header.Get("Authorization"), " ")
	if !ok || !strings.EqualFold(scheme, "Bearer") {
		return ""
	}
	return strings.TrimSpace(token)
}

// withoutCookie drops one cookie from a request's headers.
func withoutCookie(h http.Header, name string) {
	lines := h.Values("Cookie")
	if len(lines) == 0 {
		return
	}
	var kept []string
	for _, line := range lines {
		for part := range strings.SplitSeq(line, ";") {
			part = strings.TrimSpace(part)
			if part == "" || strings.HasPrefix(part, name+"=") {
				continue
			}
			kept = append(kept, part)
		}
	}
	h.Del("Cookie")
	if len(kept) > 0 {
		h.Set("Cookie", strings.Join(kept, "; "))
	}
}

func clientAddress(r *http.Request) string {
	if host, _, err := net.SplitHostPort(r.RemoteAddr); err == nil {
		return host
	}
	return r.RemoteAddr
}

func cmp(s, fallback string) string {
	if s == "" {
		return fallback
	}
	return s
}

func writeJSON(w http.ResponseWriter, status int, v any) {
	data, err := json.Marshal(v)
	if err != nil {
		http.Error(w, err.Error(), http.StatusInternalServerError)
		return
	}
	w.Header().Set("Content-Type", "application/json; charset=utf-8")
	w.WriteHeader(status)
	w.Write(append(data, '\n'))
}
