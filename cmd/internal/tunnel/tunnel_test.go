package tunnel

import (
	"bufio"
	"context"
	"crypto/tls"
	"encoding/json/v2"
	"fmt"
	"io"
	"log"
	"net/http"
	"net/http/cookiejar"
	"net/http/httptest"
	"net/url"
	"path/filepath"
	"strings"
	"testing"
	"time"

	"github.com/gorilla/websocket"
)

const testToken = "0123456789abcdef-test-token"

// cockpit is what the tests serve through the tunnel: what the relay's
// clients see of a cockpit.
func cockpit() http.Handler {
	mux := http.NewServeMux()
	mux.HandleFunc("GET /api/hello", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "application/json")
		fmt.Fprintf(w, `{"host":%q,"auth":%q,"cookie":%q,"forwarded":%q}`,
			r.Host, r.Header.Get("Authorization"), r.Header.Get("Cookie"), r.Header.Get("X-Forwarded-Host"))
	})
	mux.HandleFunc("POST /api/echo", func(w http.ResponseWriter, r *http.Request) {
		// HTTP/1 handlers read the body before they answer.
		data, _ := io.ReadAll(r.Body)
		w.Write(data)
	})
	mux.HandleFunc("GET /api/events", func(w http.ResponseWriter, r *http.Request) {
		w.Header().Set("Content-Type", "text/event-stream")
		controller := http.NewResponseController(w)
		for i := range 3 {
			fmt.Fprintf(w, "id: %d\ndata: event %d\n\n", i, i)
			controller.Flush()
			time.Sleep(20 * time.Millisecond)
		}
	})
	upgrader := websocket.Upgrader{CheckOrigin: func(*http.Request) bool { return true }}
	mux.HandleFunc("GET /api/socket", func(w http.ResponseWriter, r *http.Request) {
		ws, err := upgrader.Upgrade(w, r, nil)
		if err != nil {
			return
		}
		defer ws.Close()
		for {
			kind, data, err := ws.ReadMessage()
			if err != nil {
				return
			}
			ws.WriteMessage(kind, append([]byte("echo: "), data...))
		}
	})
	return mux
}

type rig struct {
	relay  *Relay
	server *httptest.Server
	config Config
	cancel context.CancelFunc
	served chan error
}

// start runs a relay, and, unless offline, a cockpit connected to it.
func start(t *testing.T, tlsOn, offline bool) *rig {
	t.Helper()
	r := &rig{relay: NewRelay(testToken, "test", t.Logf)}
	if tlsOn {
		cert, fp, err := SelfSigned(t.TempDir(), []string{"127.0.0.1"})
		if err != nil {
			t.Fatal(err)
		}
		r.server = httptest.NewUnstartedServer(r.relay)
		r.server.TLS = &tls.Config{Certificates: []tls.Certificate{cert}}
		// The handshakes the pin refuses are the tests'.
		r.server.Config.ErrorLog = log.New(io.Discard, "", 0)
		r.server.StartTLS()
		r.config = Config{URL: r.server.URL, Token: testToken, Fingerprint: fp}
	} else {
		r.server = httptest.NewServer(r.relay)
		r.config = Config{URL: r.server.URL, Token: testToken}
	}
	t.Cleanup(func() {
		if r.cancel != nil {
			r.cancel()
			<-r.served
		}
		r.relay.Close()
		r.server.Close()
	})
	if offline {
		return r
	}
	ctx, cancel := context.WithCancel(context.Background())
	session, err := Dial(ctx, r.config)
	if err != nil {
		cancel()
		t.Fatal(err)
	}
	r.cancel, r.served = cancel, make(chan error, 1)
	go func() { r.served <- Serve(ctx, session, cockpit()) }()
	deadline := time.Now().Add(5 * time.Second)
	for {
		if ok, _ := r.relay.Connected(); ok {
			break
		}
		if time.Now().After(deadline) {
			t.Fatal("the cockpit did not connect")
		}
		time.Sleep(10 * time.Millisecond)
	}
	return r
}

func (r *rig) client() *http.Client {
	transport := &http.Transport{TLSClientConfig: TLSConfig(r.config)}
	return &http.Client{Transport: transport, Timeout: 10 * time.Second}
}

func (r *rig) get(t *testing.T, path string, authorized bool) (*http.Response, string) {
	t.Helper()
	req, _ := http.NewRequest(http.MethodGet, r.config.Base()+path, nil)
	if authorized {
		req.Header.Set("Authorization", "Bearer "+testToken)
	}
	resp, err := r.client().Do(req)
	if err != nil {
		t.Fatal(err)
	}
	defer resp.Body.Close()
	data, _ := io.ReadAll(resp.Body)
	return resp, string(data)
}

func TestTunnelCarriesRequests(t *testing.T) {
	for _, tlsOn := range []bool{false, true} {
		t.Run(fmt.Sprintf("tls=%v", tlsOn), func(t *testing.T) {
			r := start(t, tlsOn, false)
			host := strings.TrimPrefix(strings.TrimPrefix(r.config.Base(), "http://"), "https://")

			resp, body := r.get(t, "/api/hello", true)
			if resp.StatusCode != http.StatusOK {
				t.Fatalf("status %d: %s", resp.StatusCode, body)
			}
			var hello struct {
				Host      string `json:"host"`
				Auth      string `json:"auth"`
				Cookie    string `json:"cookie"`
				Forwarded string `json:"forwarded"`
			}
			if err := json.Unmarshal([]byte(body), &hello); err != nil {
				t.Fatal(err)
			}
			if hello.Host != host || hello.Forwarded != host {
				t.Errorf("the cockpit saw host %q (forwarded %q), want %q", hello.Host, hello.Forwarded, host)
			}
			if hello.Auth != "" {
				t.Errorf("the relay's token reached the cockpit: %q", hello.Auth)
			}

			// A body goes up and comes back.
			req, _ := http.NewRequest(http.MethodPost, r.config.Base()+"/api/echo", strings.NewReader(strings.Repeat("x", 300_000)))
			req.Header.Set("Authorization", "Bearer "+testToken)
			echoed, err := r.client().Do(req)
			if err != nil {
				t.Fatal(err)
			}
			data, _ := io.ReadAll(echoed.Body)
			echoed.Body.Close()
			if len(data) != 300_000 {
				t.Errorf("echoed %d bytes, want 300000", len(data))
			}

			// Events come as they are sent.
			req, _ = http.NewRequest(http.MethodGet, r.config.Base()+"/api/events", nil)
			req.Header.Set("Authorization", "Bearer "+testToken)
			events, err := r.client().Do(req)
			if err != nil {
				t.Fatal(err)
			}
			scanner := bufio.NewScanner(events.Body)
			var got []string
			for scanner.Scan() {
				if line, ok := strings.CutPrefix(scanner.Text(), "data: "); ok {
					got = append(got, line)
				}
			}
			events.Body.Close()
			if strings.Join(got, ",") != "event 0,event 1,event 2" {
				t.Errorf("events %q", got)
			}

			// A WebSocket goes through, as the terminals' do.
			dialer := websocket.Dialer{TLSClientConfig: TLSConfig(r.config)}
			socketURL := strings.Replace(r.config.Base(), "http", "ws", 1) + "/api/socket"
			ws, _, err := dialer.Dial(socketURL, http.Header{"Authorization": {"Bearer " + testToken}})
			if err != nil {
				t.Fatal(err)
			}
			defer ws.Close()
			ws.WriteMessage(websocket.TextMessage, []byte("hi"))
			ws.SetReadDeadline(time.Now().Add(5 * time.Second))
			if _, data, err := ws.ReadMessage(); err != nil || string(data) != "echo: hi" {
				t.Errorf("socket read %q, %v", data, err)
			}
		})
	}
}

func TestRelayWantsTheToken(t *testing.T) {
	r := start(t, false, false)
	if resp, body := r.get(t, "/api/hello", false); resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("without the token: %d %s", resp.StatusCode, body)
	}
	req, _ := http.NewRequest(http.MethodGet, r.config.Base()+"/api/hello", nil)
	req.Header.Set("Authorization", "Bearer not-the-token-at-all")
	if resp, _ := r.client().Do(req); resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("with another token: %d", resp.StatusCode)
	}

	// Health says what the relay is to anyone, and more to the token.
	_, body := r.get(t, HealthPath, false)
	var health Health
	json.Unmarshal([]byte(body), &health)
	if health.Relay != Name || health.Authorized || health.Agent || health.Version != "" {
		t.Errorf("health without the token: %s", body)
	}
	if health, err := Check(context.Background(), r.config); err != nil || !health.Agent || health.Version != "test" {
		t.Errorf("check: %+v, %v", health, err)
	}
	wrong := r.config
	wrong.Token = "another-token-of-enough-length"
	if _, err := Check(context.Background(), wrong); Code(err) != "unauthorized" {
		t.Errorf("check with another token: %v", err)
	}
	if _, err := Dial(context.Background(), wrong); Code(err) != "unauthorized" {
		t.Errorf("dial with another token: %v", err)
	}
}

func TestBrowserKeepsTheTokenInACookie(t *testing.T) {
	r := start(t, false, false)
	jar, _ := cookiejar.New(nil)
	client := &http.Client{Jar: jar, Timeout: 10 * time.Second}
	resp, err := client.Get(r.config.BrowserLink() + "&x=1")
	if err != nil {
		t.Fatal(err)
	}
	resp.Body.Close()
	if resp.Request.URL.Query().Get(TokenQuery) != "" || resp.Request.URL.Query().Get("x") != "1" {
		t.Errorf("landed on %s", resp.Request.URL)
	}
	resp, err = client.Get(r.config.Base() + "/api/hello")
	if err != nil {
		t.Fatal(err)
	}
	data, _ := io.ReadAll(resp.Body)
	resp.Body.Close()
	if resp.StatusCode != http.StatusOK {
		t.Fatalf("with the cookie: %d %s", resp.StatusCode, data)
	}
	if strings.Contains(string(data), testToken) {
		t.Errorf("the relay's cookie reached the cockpit: %s", data)
	}
	if resp, err := client.Get(r.config.Base() + "/?" + TokenQuery + "=wrong-token-of-enough-length"); err != nil || resp.StatusCode != http.StatusUnauthorized {
		t.Errorf("a wrong token in the address: %v %v", resp.StatusCode, err)
	}
}

func TestRelayWithoutCockpit(t *testing.T) {
	r := start(t, false, true)
	resp, body := r.get(t, "/api/hello", true)
	if resp.StatusCode != http.StatusServiceUnavailable || !strings.Contains(body, "agent_offline") {
		t.Errorf("offline: %d %s", resp.StatusCode, body)
	}
	if health, err := Check(context.Background(), r.config); err != nil || health.Agent {
		t.Errorf("check: %+v %v", health, err)
	}
}

func TestCockpitThatLeavesAndComesBack(t *testing.T) {
	r := start(t, false, false)
	r.cancel()
	<-r.served
	r.cancel = nil
	deadline := time.Now().Add(5 * time.Second)
	for ok, _ := r.relay.Connected(); ok; ok, _ = r.relay.Connected() {
		if time.Now().After(deadline) {
			t.Fatal("the relay did not notice the cockpit leave")
		}
		time.Sleep(10 * time.Millisecond)
	}
	if resp, body := r.get(t, "/api/hello", true); resp.StatusCode != http.StatusServiceUnavailable {
		t.Errorf("after the cockpit left: %d %s", resp.StatusCode, body)
	}
	ctx, cancel := context.WithCancel(context.Background())
	session, err := Dial(ctx, r.config)
	if err != nil {
		t.Fatal(err)
	}
	r.cancel, r.served = cancel, make(chan error, 1)
	go func() { r.served <- Serve(ctx, session, cockpit()) }()
	for ok, _ := r.relay.Connected(); !ok; ok, _ = r.relay.Connected() {
		time.Sleep(10 * time.Millisecond)
	}
	if resp, body := r.get(t, "/api/hello", true); resp.StatusCode != http.StatusOK {
		t.Errorf("after the cockpit came back: %d %s", resp.StatusCode, body)
	}
}

func TestPinnedCertificate(t *testing.T) {
	r := start(t, true, true)
	other := r.config
	other.Fingerprint = strings.Repeat("ab", 32)
	if _, err := Check(context.Background(), other); Code(err) != "unreachable" || !strings.Contains(err.Error(), "not the one pinned") {
		t.Errorf("another pin: %v", err)
	}
	if _, err := Dial(context.Background(), other); err == nil || !strings.Contains(err.Error(), "not the one pinned") {
		t.Errorf("dial with another pin: %v", err)
	}
	if _, err := Check(context.Background(), r.config); err != nil {
		t.Errorf("the right pin: %v", err)
	}
}

func TestConfig(t *testing.T) {
	path := filepath.Join(t.TempDir(), "tunnel.json")
	c := Config{URL: "https://relay.example:8420/", Token: testToken, Fingerprint: strings.ToUpper(strings.Repeat("ab:", 31) + "ab"), Host: "me@relay"}
	if err := SaveConfig(path, c); err != nil {
		t.Fatal(err)
	}
	got, err := LoadConfig(path)
	if err != nil {
		t.Fatal(err)
	}
	if got.Fingerprint != strings.Repeat("ab", 32) || got.Base() != "https://relay.example:8420" || got.Host != "me@relay" {
		t.Errorf("loaded %+v", got)
	}
	link, _ := url.Parse(got.PairLink())
	if link.Scheme != PairScheme || link.Host != "pair" || link.Query().Get("url") != got.Base() || link.Query().Get("token") != testToken || link.Query().Get("fp") != got.Fingerprint {
		t.Errorf("pair link %s", got.PairLink())
	}
	for _, bad := range []Config{
		{URL: "relay.example", Token: testToken},
		{URL: "ftp://relay.example", Token: testToken},
		{URL: "https://relay.example/path", Token: testToken},
		{URL: "https://relay.example", Token: "short"},
		{URL: "https://relay.example", Token: testToken, Fingerprint: "abc"},
	} {
		if bad.Validate() == nil {
			t.Errorf("%+v passed", bad)
		}
	}
}
