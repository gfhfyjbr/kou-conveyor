package main

import (
	"bytes"
	"encoding/base64"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"math"
	"net/http"
	"net/http/httptest"
	"net/url"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode/utf8"

	"github.com/gorilla/websocket"
)

// unsetVoice leaves the providers' keys to voice.json.
func unsetVoice(t *testing.T) {
	t.Helper()
	for _, p := range voiceProviders {
		for _, name := range p.Env {
			t.Setenv(name, "")
			os.Unsetenv(name)
		}
	}
}

// voiceWorkspaces are the workspaces the cockpit lists.
func (h *harness) voiceWorkspaces() []workspaceView {
	h.Helper()
	res, err := http.Get(h.http.URL + "/api/workspaces")
	if err != nil {
		h.Fatal(err)
	}
	defer res.Body.Close()
	var list []workspaceView
	if err := jsonRead(res, &list); err != nil {
		h.Fatal(err)
	}
	return list
}

// dictate takes dictation through the cockpit as a composer does: it sends
// the audio and the finish, and reads what the cockpit says until it closes
// the socket.
func (h *harness) dictate(query string, pcm []byte) []map[string]any {
	h.Helper()
	conn, _, err := websocket.DefaultDialer.Dial("ws"+strings.TrimPrefix(h.http.URL, "http")+"/api/voice/socket"+query, nil)
	if err != nil {
		h.Fatal(err)
	}
	defer conn.Close()
	for i := 0; i < len(pcm); i += 3200 {
		conn.WriteMessage(websocket.BinaryMessage, pcm[i:min(len(pcm), i+3200)])
	}
	conn.WriteMessage(websocket.TextMessage, []byte(`{"type":"finish"}`))
	conn.SetReadDeadline(time.Now().Add(20 * time.Second))
	var messages []map[string]any
	for {
		_, data, err := conn.ReadMessage()
		if err != nil {
			if !websocket.IsCloseError(err, websocket.CloseNormalClosure) {
				h.Fatalf("the socket broke: %v, after %v", err, messages)
			}
			return messages
		}
		var m map[string]any
		if err := json.Unmarshal(data, &m); err != nil {
			h.Fatal(err)
		}
		messages = append(messages, m)
	}
}

// lastOf is the last message of a type, nil for none.
func lastOf(messages []map[string]any, kind string) map[string]any {
	for _, m := range slices.Backward(messages) {
		if m["type"] == kind {
			return m
		}
	}
	return nil
}

// speech is a second of a tone as the composers send it.
func speech(seconds float64) []byte {
	var pcm []byte
	for i := range int(seconds * voiceRate) {
		v := int16(8000 * math.Sin(float64(i)*2*math.Pi*220/voiceRate))
		pcm = append(pcm, byte(v), byte(uint16(v)>>8))
	}
	return pcm
}

// voiceMock is a provider of the tests': it records what the cockpit sent.
type voiceMock struct {
	url string

	mu       sync.Mutex
	query    url.Values
	header   http.Header
	first    map[string]any   // the stream's first message
	messages []map[string]any // its other JSON messages
	audio    int              // bytes of audio
	form     map[string]string
	silent   bool // it is never sure of what it heard
}

func (m *voiceMock) record(r *http.Request) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.query, m.header = r.URL.Query(), r.Header.Clone()
}

func (m *voiceMock) note(message map[string]any) {
	m.mu.Lock()
	defer m.mu.Unlock()
	if m.first == nil {
		m.first = message
	} else {
		m.messages = append(m.messages, message)
	}
}

func (m *voiceMock) heard(n int) {
	m.mu.Lock()
	defer m.mu.Unlock()
	m.audio += n
}

func (m *voiceMock) isSilent() bool {
	m.mu.Lock()
	defer m.mu.Unlock()
	return m.silent
}

// types are the values of a key of the messages after the first; m.mu is
// held.
func (m *voiceMock) types(key string) []string {
	var out []string
	for _, message := range m.messages {
		out = append(out, fmt.Sprint(message[key]))
	}
	return out
}

var mockUpgrader = websocket.Upgrader{CheckOrigin: func(*http.Request) bool { return true }}

func writeMock(conn *websocket.Conn, v any) {
	data, _ := json.Marshal(v)
	conn.WriteMessage(websocket.TextMessage, data)
}

func readMock(data []byte) map[string]any {
	var m map[string]any
	json.Unmarshal(data, &m)
	return m
}

// voiceMocks are the seven providers, each speaking its protocol: each
// hears "привет мир", a guess first where it streams.
func voiceMocks(t *testing.T) map[string]*voiceMock {
	t.Helper()
	mocks := make(map[string]*voiceMock)
	serve := func(id string, handler func(m *voiceMock, w http.ResponseWriter, r *http.Request)) {
		m := &voiceMock{}
		srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
			m.record(r)
			handler(m, w, r)
		}))
		t.Cleanup(srv.Close)
		m.url = srv.URL
		mocks[id] = m
	}
	allow := func(w http.ResponseWriter, ok bool) {
		w.Header().Set("Content-Type", "application/json")
		if !ok {
			w.WriteHeader(http.StatusUnauthorized)
			fmt.Fprint(w, `{"message":"Invalid API key."}`)
			return
		}
		fmt.Fprint(w, `{"data":[]}`)
	}

	serve("soniox", func(m *voiceMock, w http.ResponseWriter, r *http.Request) {
		authorized := r.Header.Get("Authorization") == "Bearer key-soniox"
		switch r.URL.Path {
		case "/v1/models":
			allow(w, authorized)
		case "/transcribe-websocket":
			conn, err := mockUpgrader.Upgrade(w, r, nil)
			if err != nil {
				return
			}
			defer conn.Close()
			_, data, err := conn.ReadMessage()
			if err != nil {
				return
			}
			m.note(readMock(data))
			if !authorized {
				writeMock(conn, map[string]any{"error_code": 401, "error_message": "Invalid API key."})
				return
			}
			guessed := false
			for {
				kind, data, err := conn.ReadMessage()
				if err != nil {
					return
				}
				switch {
				case kind == websocket.BinaryMessage:
					m.heard(len(data))
					if !guessed {
						guessed = true
						writeMock(conn, map[string]any{"tokens": []map[string]any{{"text": "при", "is_final": false}}})
					}
				case len(data) == 0:
					writeMock(conn, map[string]any{"tokens": []any{}, "finished": true})
					return
				default:
					m.note(readMock(data))
					if !m.isSilent() {
						writeMock(conn, map[string]any{"tokens": []map[string]any{
							{"text": "при", "is_final": true}, {"text": "вет", "is_final": true}, {"text": " мир", "is_final": true}, {"text": "<fin>", "is_final": true},
						}})
					}
				}
			}
		}
	})

	serve("deepgram", func(m *voiceMock, w http.ResponseWriter, r *http.Request) {
		authorized := r.Header.Get("Authorization") == "Token key-deepgram"
		switch r.URL.Path {
		case "/v1/projects":
			allow(w, authorized)
		case "/v1/listen":
			if !authorized {
				http.Error(w, `{"err_code":"INVALID_AUTH","err_msg":"Invalid credentials."}`, http.StatusUnauthorized)
				return
			}
			conn, err := mockUpgrader.Upgrade(w, r, nil)
			if err != nil {
				return
			}
			defer conn.Close()
			m.note(map[string]any{"opened": true})
			results := func(text string, final bool) map[string]any {
				return map[string]any{"type": "Results", "is_final": final, "channel": map[string]any{"alternatives": []map[string]any{{"transcript": text}}}}
			}
			guessed := false
			for {
				kind, data, err := conn.ReadMessage()
				if err != nil {
					return
				}
				if kind == websocket.BinaryMessage {
					m.heard(len(data))
					if !guessed {
						guessed = true
						writeMock(conn, results("привет", false))
					}
					continue
				}
				message := readMock(data)
				m.note(message)
				if message["type"] == "CloseStream" {
					writeMock(conn, results("привет", true))
					writeMock(conn, results("мир", true))
					writeMock(conn, map[string]any{"type": "Metadata", "request_id": "x"})
					conn.WriteMessage(websocket.CloseMessage, websocket.FormatCloseMessage(websocket.CloseNormalClosure, ""))
					return
				}
			}
		}
	})

	serve("elevenlabs", func(m *voiceMock, w http.ResponseWriter, r *http.Request) {
		authorized := r.Header.Get("xi-api-key") == "key-elevenlabs"
		switch r.URL.Path {
		case "/v1/single-use-token/realtime_scribe":
			allow(w, authorized && r.Method == http.MethodPost)
		case "/v1/speech-to-text/realtime":
			conn, err := mockUpgrader.Upgrade(w, r, nil)
			if err != nil {
				return
			}
			defer conn.Close()
			if !authorized {
				writeMock(conn, map[string]any{"message_type": "auth_error", "error": "Invalid API key"})
				return
			}
			writeMock(conn, map[string]any{"message_type": "session_started", "session_id": "s"})
			m.note(map[string]any{"opened": true})
			guessed := false
			for {
				_, data, err := conn.ReadMessage()
				if err != nil {
					return
				}
				message := readMock(data)
				pcm, _ := base64.StdEncoding.DecodeString(fmt.Sprint(message["audio_base_64"]))
				m.heard(len(pcm))
				delete(message, "audio_base_64")
				m.note(message)
				if message["commit"] == true {
					writeMock(conn, map[string]any{"message_type": "committed_transcript", "text": "привет мир"})
				} else if !guessed {
					guessed = true
					writeMock(conn, map[string]any{"message_type": "partial_transcript", "text": "привет"})
				}
			}
		}
	})

	serve("openai", func(m *voiceMock, w http.ResponseWriter, r *http.Request) {
		authorized := r.Header.Get("Authorization") == "Bearer key-openai"
		switch r.URL.Path {
		case "/v1/models":
			allow(w, authorized)
		case "/v1/realtime":
			conn, err := mockUpgrader.Upgrade(w, r, nil)
			if err != nil {
				return
			}
			defer conn.Close()
			if !authorized {
				writeMock(conn, map[string]any{"type": "error", "error": map[string]any{"code": "invalid_api_key", "message": "Incorrect API key provided"}})
				return
			}
			writeMock(conn, map[string]any{"type": "session.created"})
			deltas := 0
			for {
				_, data, err := conn.ReadMessage()
				if err != nil {
					return
				}
				message := readMock(data)
				if message["type"] == "input_audio_buffer.append" {
					pcm, _ := base64.StdEncoding.DecodeString(fmt.Sprint(message["audio"]))
					m.heard(len(pcm))
					if deltas < 2 {
						writeMock(conn, map[string]any{"type": "conversation.item.input_audio_transcription.delta", "delta": []string{"при", "вет"}[deltas]})
						deltas++
					}
					continue
				}
				m.note(message)
				if message["type"] == "input_audio_buffer.commit" {
					writeMock(conn, map[string]any{"type": "input_audio_buffer.committed"})
					writeMock(conn, map[string]any{"type": "conversation.item.input_audio_transcription.completed", "transcript": "привет мир"})
				}
			}
		}
	})

	serve("gemini", func(m *voiceMock, w http.ResponseWriter, r *http.Request) {
		authorized := r.Header.Get("x-goog-api-key") == "key-gemini"
		switch r.URL.Path {
		case "/v1beta/models":
			if !authorized {
				http.Error(w, `{"error":{"code":400,"message":"API key not valid. Please pass a valid API key."}}`, http.StatusBadRequest)
				return
			}
			allow(w, true)
		case "/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent":
			conn, err := mockUpgrader.Upgrade(w, r, nil)
			if err != nil {
				return
			}
			defer conn.Close()
			_, data, err := conn.ReadMessage()
			if err != nil {
				return
			}
			m.note(readMock(data))
			if !authorized {
				conn.WriteMessage(websocket.CloseMessage, websocket.FormatCloseMessage(1007, "API key not valid. Please pass a valid API key."))
				return
			}
			writeMock(conn, map[string]any{"setupComplete": map[string]any{}})
			guessed := false
			for {
				_, data, err := conn.ReadMessage()
				if err != nil {
					return
				}
				message := readMock(data)
				input, _ := message["realtimeInput"].(map[string]any)
				if audio, ok := input["audio"].(map[string]any); ok {
					pcm, _ := base64.StdEncoding.DecodeString(fmt.Sprint(audio["data"]))
					m.heard(len(pcm))
					if !guessed {
						guessed = true
						m.note(map[string]any{"kind": "audio", "mimeType": audio["mimeType"]})
						writeMock(conn, map[string]any{"serverContent": map[string]any{"interimInputTranscription": map[string]any{"text": "привет"}}})
					}
					continue
				}
				for kind := range input {
					m.note(map[string]any{"kind": kind})
					if kind == "activityEnd" {
						writeMock(conn, map[string]any{"serverContent": map[string]any{"inputTranscription": map[string]any{"text": "привет мир"}}})
						writeMock(conn, map[string]any{"serverContent": map[string]any{"turnComplete": true}})
					}
				}
			}
		}
	})

	serve("groq", func(m *voiceMock, w http.ResponseWriter, r *http.Request) {
		authorized := r.Header.Get("Authorization") == "Bearer key-groq"
		switch r.URL.Path {
		case "/openai/v1/models":
			allow(w, authorized)
		case "/openai/v1/audio/transcriptions":
			if !authorized {
				allow(w, false)
				return
			}
			if err := r.ParseMultipartForm(8 << 20); err != nil {
				http.Error(w, err.Error(), http.StatusBadRequest)
				return
			}
			form := map[string]string{}
			for name, values := range r.MultipartForm.Value {
				form[name] = values[0]
			}
			if file, _, err := r.FormFile("file"); err == nil {
				data, _ := io.ReadAll(file)
				form["file"] = string(data[:4])
				m.heard(len(data) - 44)
			}
			m.mu.Lock()
			m.form = form
			m.mu.Unlock()
			fmt.Fprint(w, `{"text":" привет мир "}`)
		}
	})

	serve("yandex", func(m *voiceMock, w http.ResponseWriter, r *http.Request) {
		if r.Header.Get("Authorization") != "Api-Key key-yandex" {
			http.Error(w, `{"error_code":"UNAUTHORIZED","error_message":"The API key is invalid"}`, http.StatusUnauthorized)
			return
		}
		data, _ := io.ReadAll(r.Body)
		m.heard(len(data))
		text := "привет мир"
		if len(data) < voiceRate*2/2 {
			text = "" // the check's silence
		}
		fmt.Fprintf(w, `{"result":%q}`, text)
	})
	return mocks
}

// useMocks points the cockpit at the mocks.
func (h *harness) useMocks(mocks map[string]*voiceMock) {
	h.server.voice.bases = make(map[string]string)
	for id, m := range mocks {
		h.server.voice.bases[id] = m.url
	}
}

func TestVoiceNeedsTheKey(t *testing.T) {
	unsetVoice(t)
	h := newHarness(t)
	// A workspace's .env is its runs', not the cockpit's: a key there is
	// not dictation's.
	writeFiles(t, h.server.opt.Workspace, map[string]string{".env": "SONIOX_API_KEY=test-key\nDEEPGRAM_API_KEY=test-key\n"})
	messages := h.dictate("?languages=ru-RU", speech(0.2))
	if len(messages) != 1 || messages[0]["type"] != "error" || messages[0]["setup"] != true || !strings.Contains(fmt.Sprint(messages[0]["message"]), "SONIOX_API_KEY") {
		t.Fatalf("without a key: %v", messages)
	}
	// The one named: its own variable.
	if messages := h.dictate("?provider=deepgram", nil); len(messages) != 1 || messages[0]["setup"] != true || !strings.Contains(fmt.Sprint(messages[0]["message"]), "DEEPGRAM_API_KEY") {
		t.Fatalf("Deepgram without a key: %v", messages)
	}
	if messages := h.dictate("?provider=whisper.cpp", nil); len(messages) != 1 || messages[0]["setup"] != false || !strings.Contains(fmt.Sprint(messages[0]["message"]), "no provider") {
		t.Fatalf("an unknown provider: %v", messages)
	}
	_, view := h.do("GET", "/api/voice", "")
	if view["set"] != false || view["provider"] != "" || view["path"] != filepath.Join(filepath.Dir(h.server.opt.SettingsFile), "voice.json") {
		t.Fatalf("the view without a key: %v", view)
	}
	var ids []string
	for _, p := range view["providers"].([]any) {
		p := p.(map[string]any)
		if p["set"] != false || p["name"] == "" || p["console"] == "" || len(p["env"].([]any)) == 0 {
			t.Errorf("a provider without a key: %v", p)
		}
		ids = append(ids, fmt.Sprint(p["id"]))
	}
	if fmt.Sprint(ids) != "[soniox deepgram elevenlabs openai gemini groq yandex]" {
		t.Errorf("the providers: %v", ids)
	}
	// The plugin runs all the same: its button asks for a key.
	_, listing := h.do("GET", "/api/plugins", "")
	if voice := pluginNamed(t, listing, "voice"); voice["active"] != true {
		t.Fatalf("the voice plugin without a key: %v", voice)
	}
	if list := h.voiceWorkspaces(); len(list) == 0 || list[0].Voice {
		t.Fatalf("workspaces without a key: %+v", list)
	}
}

func TestVoiceKeys(t *testing.T) {
	unsetVoice(t)
	h := newHarness(t)
	h.useMocks(voiceMocks(t))
	file := h.server.voicePath()

	// A key its provider refuses is not kept.
	for _, id := range []string{"soniox", "deepgram", "elevenlabs", "openai", "gemini", "groq", "yandex"} {
		res, body := h.do("PUT", "/api/voice", fmt.Sprintf(`{"keys":{%q:"wrong-key"}}`, id))
		if res.StatusCode != http.StatusBadRequest || !strings.Contains(fmt.Sprint(body["error"]), "refused the key") {
			t.Errorf("saving a wrong key of %s: %d %v", id, res.StatusCode, body)
		}
	}
	if _, err := os.Stat(file); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("a wrong key was kept: %v", err)
	}
	for _, bad := range []string{`{}`, `{"keys":{"whisper":"key"}}`, `{"keys":{"soniox":"two words"}}`, `{"settings":{"provider":"whisper"}}`} {
		if res, body := h.do("PUT", "/api/voice", bad); res.StatusCode != http.StatusBadRequest {
			t.Errorf("saving %s: %d %v", bad, res.StatusCode, body)
		}
	}

	// The cockpit keeps the keys their providers take, as pasted, for the
	// user alone; the page learns that there is one, never what it is.
	res, body := h.do("PUT", "/api/voice", `{"keys":{"deepgram":"  \"key-deepgram\"\n","groq":"key-groq"}}`)
	if res.StatusCode != http.StatusOK || body["set"] != true || body["provider"] != "deepgram" || strings.Contains(fmt.Sprint(body), "key-") {
		t.Fatalf("saving keys: %d %v", res.StatusCode, body)
	}
	if info, err := os.Stat(file); err != nil || info.Mode().Perm() != 0o600 {
		t.Fatalf("voice.json: %v %v", info, err)
	}
	if data, _ := os.ReadFile(file); !strings.Contains(string(data), `"key-deepgram"`) || !strings.Contains(string(data), `"key-groq"`) {
		t.Fatalf("voice.json: %s", data)
	}
	provider := func(view map[string]any, id string) map[string]any {
		for _, p := range view["providers"].([]any) {
			if p := p.(map[string]any); p["id"] == id {
				return p
			}
		}
		t.Fatalf("no %s in %v", id, view)
		return nil
	}
	if p := provider(body, "groq"); p["set"] != true || p["from"] != voiceFromFile || p["hint"] != nil {
		t.Errorf("groq's key: %v", p)
	}
	if p := provider(body, "soniox"); p["set"] != false {
		t.Errorf("soniox without a key: %v", p)
	}
	if list := h.voiceWorkspaces(); len(list) == 0 || !list[0].Voice {
		t.Fatalf("workspaces with a key: %+v", list)
	}

	// The provider chosen listens, though another comes first; without its
	// key, dictation waits for one.
	if res, body := h.do("PUT", "/api/voice", `{"settings":{"provider":"groq"}}`); res.StatusCode != http.StatusOK || body["provider"] != "groq" || body["set"] != true {
		t.Fatalf("choosing groq: %d %v", res.StatusCode, body)
	}
	if res, body := h.do("PUT", "/api/voice", `{"settings":{"provider":"openai"}}`); res.StatusCode != http.StatusOK || body["provider"] != "openai" || body["set"] != false {
		t.Fatalf("choosing openai: %d %v", res.StatusCode, body)
	}
	if list := h.voiceWorkspaces(); len(list) == 0 || list[0].Voice {
		t.Fatalf("workspaces whose provider has no key: %+v", list)
	}

	// The environment's key wins, and is not to be replaced from the page.
	t.Setenv("OPENAI_API_KEY", "environment-key-1234")
	_, view := h.do("GET", "/api/voice", "")
	if p := provider(view, "openai"); view["set"] != true || p["from"] != voiceFromEnvironment || p["hint"] != "…1234" {
		t.Fatalf("the environment's key: %v", view)
	}
	if res, body := h.do("PUT", "/api/voice", `{"keys":{"openai":"key-openai"}}`); res.StatusCode != http.StatusConflict {
		t.Fatalf("saving over the environment's key: %d %v", res.StatusCode, body)
	}
	unsetVoice(t)

	// A key refused once saved is no 401 of the cockpit's, which would tell
	// the iPhone to pair again: the socket says so, and asks for another.
	if err := saveVoiceFile(file, voiceFile{Keys: map[string]string{"deepgram": "wrong-key"}, Settings: &voiceSettings{Provider: "deepgram"}}); err != nil {
		t.Fatal(err)
	}
	messages := h.dictate("", speech(0.2))
	if last := messages[len(messages)-1]; last["type"] != "error" || last["setup"] != true || !strings.Contains(fmt.Sprint(last["message"]), "Invalid credentials") {
		t.Fatalf("a wrong key: %v", messages)
	}
	// One of the environment's is to be changed there.
	t.Setenv("DEEPGRAM_API_KEY", "wrong-key")
	messages = h.dictate("", speech(0.2))
	if last := messages[len(messages)-1]; last["type"] != "error" || last["setup"] != false || !strings.Contains(fmt.Sprint(last["message"]), "DEEPGRAM_API_KEY") {
		t.Fatalf("a wrong key of the environment: %v", messages)
	}
	unsetVoice(t)

	// The key of the days of Soniox alone still serves, and moves among the
	// others once the file is saved.
	if err := os.WriteFile(file, []byte(`{"soniox_api_key":"key-soniox"}`), 0o600); err != nil {
		t.Fatal(err)
	}
	if _, view := h.do("GET", "/api/voice", ""); view["provider"] != "soniox" || view["set"] != true || provider(view, "soniox")["from"] != voiceFromFile {
		t.Fatalf("the old key: %v", view)
	}
	h.do("PUT", "/api/voice", `{"settings":{"languages":["en"]}}`)
	if data, _ := os.ReadFile(file); strings.Contains(string(data), "soniox_api_key") || !strings.Contains(string(data), `"soniox":"key-soniox"`) {
		t.Fatalf("voice.json after the old key moved: %s", data)
	}

	// Removed, it is gone; the settings stay. Without them, the file goes.
	if res, body := h.do("DELETE", "/api/voice", ""); res.StatusCode != http.StatusBadRequest {
		t.Fatalf("removing no one's key: %d %v", res.StatusCode, body)
	}
	res, body = h.do("DELETE", "/api/voice?provider=soniox", "")
	if res.StatusCode != http.StatusOK || body["set"] != false || fmt.Sprint(body["settings"].(map[string]any)["languages"]) != "[en]" {
		t.Fatalf("removing the key: %d %v", res.StatusCode, body)
	}
	if data, err := os.ReadFile(file); err != nil || strings.Contains(string(data), "key-soniox") {
		t.Fatalf("voice.json after removing the key: %s %v", data, err)
	}
	if err := saveVoiceFile(file, voiceFile{Keys: map[string]string{"groq": "key-groq"}}); err != nil {
		t.Fatal(err)
	}
	h.do("DELETE", "/api/voice?provider=groq", "")
	if _, err := os.Stat(file); !errors.Is(err, fs.ErrNotExist) {
		t.Fatalf("voice.json after removing the last key without settings: %v", err)
	}
}

func TestVoiceSettings(t *testing.T) {
	unsetVoice(t)
	h := newHarness(t)
	// The settings are the defaults until saved.
	if _, view := h.do("GET", "/api/voice", ""); fmt.Sprint(view["settings"]) != "map[coding:true files:true languages:[] provider: strict:false swearing:true words:[]]" {
		t.Fatalf("the default settings: %v", view["settings"])
	}
	many := make([]string, maxVoiceWords+1)
	for i := range many {
		many[i] = fmt.Sprintf("word%d", i)
	}
	tooMany, _ := json.Marshal(map[string]any{"settings": map[string]any{"words": many}})
	for _, bad := range []string{`{"settings":{"languages":["tlh"]}}`, string(tooMany)} {
		if res, body := h.do("PUT", "/api/voice", bad); res.StatusCode != http.StatusBadRequest {
			t.Errorf("saving %.60s: %d %v", bad, res.StatusCode, body)
		}
	}
	res, body := h.do("PUT", "/api/voice", `{"settings":{"provider":"gemini","languages":["de-DE","ru","de"],"strict":true,"swearing":true,"files":false,"coding":false,"words":["  Kou   Conveyor ","Soniox","Soniox",""]}}`)
	if res.StatusCode != http.StatusOK || body["set"] != false || fmt.Sprint(body["settings"]) != "map[coding:false files:false languages:[de ru] provider:gemini strict:true swearing:true words:[Kou Conveyor Soniox]]" {
		t.Fatalf("saving settings: %d %v", res.StatusCode, body)
	}
}

func TestVoiceProviders(t *testing.T) {
	unsetVoice(t)
	h := newHarness(t)
	mocks := voiceMocks(t)
	h.useMocks(mocks)
	// The names are listed for the first take, however slow the race
	// detector makes it.
	wait := voiceTermsWait
	voiceTermsWait = 20 * time.Second
	t.Cleanup(func() { voiceTermsWait = wait })
	workspace := h.server.opt.Workspace
	writeFiles(t, workspace, map[string]string{
		"ios/KouConveyor/Views/SessionScreen.swift": "",
		"cmd/web/plugins/composer/web/composer.js":  "",
		"cmd/web/voice_test.go":                     "",
		"third_party/huge/Vendored.go":              "",
	})
	keys := map[string]string{}
	for id := range mocks {
		keys[id] = "key-" + id
	}
	pcm := speech(1)
	for _, p := range voiceProviders {
		t.Run(p.ID, func(t *testing.T) {
			m := mocks[p.ID]
			settings := defaultVoiceSettings()
			settings.Provider, settings.Words = p.ID, []string{"Kou Conveyor"}
			if err := saveVoiceFile(h.server.voicePath(), voiceFile{Keys: keys, Settings: &settings}); err != nil {
				t.Fatal(err)
			}
			messages := h.dictate("?languages=ru-RU,en-US", pcm)
			if ready := messages[0]; ready["type"] != "ready" || ready["provider"] != p.ID || ready["live"] != p.Live {
				t.Fatalf("the first message: %v", messages)
			}
			text := lastOf(messages, "text")
			if text == nil || text["final"] != "привет мир" || text["partial"] != "" || messages[len(messages)-1]["type"] != "done" {
				t.Fatalf("what was heard: %v", messages)
			}
			// The live ones guess as the words are said.
			guessed := slices.ContainsFunc(messages, func(m map[string]any) bool { return m["type"] == "text" && m["partial"] != "" })
			if guessed != p.Live {
				t.Errorf("guesses %v: %v", guessed, messages)
			}
			m.mu.Lock()
			defer m.mu.Unlock()
			want := len(pcm)
			switch p.ID {
			case "elevenlabs":
				want += voiceRate / 10 * 2 // the silence of the commit
			case "openai":
				want = len(pcm) * openAIRate / voiceRate
			}
			if m.audio < want-8 || m.audio > want+8 {
				t.Errorf("%d bytes of audio reached it, not %d", m.audio, want)
			}
			check := func(terms []string, n int) {
				t.Helper()
				if len(terms) == 0 || len(terms) > n || terms[0] != "Kou Conveyor" || !slices.Contains(terms, "SessionScreen") || !slices.Contains(terms, "заебал") ||
					slices.Contains(terms, "Vendored.go") || slices.Contains(terms, "voice_test.go") {
					t.Errorf("%d terms: %q", len(terms), terms)
				}
			}
			strs := func(v any) (out []string) {
				list, _ := v.([]any)
				for _, s := range list {
					out = append(out, fmt.Sprint(s))
				}
				return out
			}
			switch p.ID {
			case "soniox":
				config := m.first
				if config["model"] != sonioxModel || config["audio_format"] != "pcm_s16le" || config["sample_rate"] != 16000.0 || fmt.Sprint(config["language_hints"]) != "[ru en]" {
					t.Errorf("config: %v", config)
				}
				context := config["context"].(map[string]any)
				terms := strs(context["terms"])
				check(terms, maxVoiceTerms)
				// Russian is spoken: its swearing comes before the names, which
				// make English of it on fast speech otherwise; the topic is code.
				if i, j := slices.Index(terms, "заебал"), slices.Index(terms, "SessionScreen"); i > j || !slices.Contains(terms, "composer.js") || !slices.Contains(terms, filepath.Base(workspace)) {
					t.Errorf("terms: %q", terms)
				}
				if context["general"] == nil || config["language_hints_strict"] != nil {
					t.Errorf("context: %v", config)
				}
				if fmt.Sprint(m.types("type")) != "[finalize]" || m.header.Get("Authorization") != "Bearer key-soniox" {
					t.Errorf("messages %v", m.types("type"))
				}
			case "deepgram":
				q := m.query
				if q.Get("model") != "nova-3" || q.Get("language") != "multi" || q.Get("encoding") != "linear16" || q.Get("sample_rate") != "16000" || q.Get("interim_results") != "true" {
					t.Errorf("query: %v", q)
				}
				check(q["keyterm"], 100)
				size := 0
				for _, term := range q["keyterm"] {
					size += len(term) + 1
				}
				if size > 1500 {
					t.Errorf("keyterms of %d bytes", size)
				}
				if fmt.Sprint(m.types("type")) != "[CloseStream]" {
					t.Errorf("messages %v", m.types("type"))
				}
			case "elevenlabs":
				q := m.query
				if q.Get("model_id") != "scribe_v2_realtime" || q.Get("audio_format") != "pcm_16000" || q.Get("commit_strategy") != "manual" || q.Get("language_code") != "ru" {
					t.Errorf("query: %v", q)
				}
				check(q["keyterms"], 50)
				for _, term := range q["keyterms"] {
					if utf8.RuneCountInString(term) > 20 {
						t.Errorf("keyterm %q is too long", term)
					}
				}
				commits := m.types("commit")
				if commits[len(commits)-1] != "true" || slices.Index(commits, "true") != len(commits)-1 || m.messages[0]["sample_rate"] != 16000.0 {
					t.Errorf("chunks: %v %v", commits, m.messages[0])
				}
			case "openai":
				if m.query.Get("intent") != "transcription" {
					t.Errorf("query: %v", m.query)
				}
				session := m.first["session"].(map[string]any)
				input := session["audio"].(map[string]any)["input"].(map[string]any)
				transcription := input["transcription"].(map[string]any)
				if m.first["type"] != "session.update" || session["type"] != "transcription" || fmt.Sprint(input["format"]) != "map[rate:24000 type:audio/pcm]" ||
					transcription["model"] != "gpt-live-transcribe" || fmt.Sprint(transcription["languages"]) != "[ru en]" ||
					!strings.Contains(fmt.Sprint(transcription["prompt"]), filepath.Base(workspace)) {
					t.Errorf("session: %v", m.first)
				}
				if turn, ok := input["turn_detection"]; !ok || turn != nil {
					t.Errorf("turn detection: %v", input)
				}
				check(strs(transcription["keywords"]), 100)
				if fmt.Sprint(m.types("type")) != "[input_audio_buffer.commit]" {
					t.Errorf("messages %v", m.types("type"))
				}
			case "gemini":
				setup := m.first["setup"].(map[string]any)
				transcription := setup["inputAudioTranscription"].(map[string]any)
				if setup["model"] != "models/gemini-3.5-transcribe-live" || fmt.Sprint(transcription["languageCodes"]) != "[ru-RU en-US]" ||
					fmt.Sprint(setup["realtimeInputConfig"]) != "map[automaticActivityDetection:map[disabled:true]]" {
					t.Errorf("setup: %v", setup)
				}
				check(strs(transcription["customVocabulary"]), 100)
				if fmt.Sprint(m.types("kind")) != "[activityStart audio activityEnd]" || m.messages[1]["mimeType"] != "audio/pcm;rate=16000" {
					t.Errorf("messages %v", m.messages)
				}
			case "groq":
				form := m.form
				if form["model"] != "whisper-large-v3" || form["file"] != "RIFF" || form["language"] != "" ||
					!strings.Contains(form["prompt"], "Kou Conveyor") || !strings.Contains(form["prompt"], "SessionScreen") || !strings.Contains(form["prompt"], filepath.Base(workspace)) {
					t.Errorf("form: %v", form)
				}
			case "yandex":
				q := m.query
				if q.Get("lang") != "ru-RU" || q.Get("format") != "lpcm" || q.Get("sampleRateHertz") != "16000" || q.Get("profanityFilter") != "false" {
					t.Errorf("query: %v", q)
				}
			}
		})
	}

	// A take with nothing said ends with nothing.
	settings := defaultVoiceSettings()
	settings.Provider = "groq"
	saveVoiceFile(h.server.voicePath(), voiceFile{Keys: keys, Settings: &settings})
	if messages := h.dictate("", nil); len(messages) != 2 || messages[1]["type"] != "done" {
		t.Fatalf("nothing said: %v", messages)
	}

	// Another provider than the settings', for a take.
	messages := h.dictate("?provider=yandex", pcm)
	if messages[0]["provider"] != "yandex" || lastOf(messages, "text")["final"] != "привет мир" {
		t.Fatalf("yandex for a take: %v", messages)
	}

	// A provider never sure of the take: what it guessed is the text, in a
	// while.
	soniox := voiceProviderOf("soniox")
	sure := soniox.Wait
	soniox.Wait = 300 * time.Millisecond
	t.Cleanup(func() { soniox.Wait = sure })
	mocks["soniox"].mu.Lock()
	mocks["soniox"].silent = true
	mocks["soniox"].mu.Unlock()
	settings.Provider = "soniox"
	saveVoiceFile(h.server.voicePath(), voiceFile{Keys: keys, Settings: &settings})
	messages = h.dictate("", pcm)
	if text := lastOf(messages, "text"); text == nil || text["final"] != "при" || messages[len(messages)-1]["type"] != "done" {
		t.Fatalf("a provider never sure: %v", messages)
	}
	// A key refused on the stream: the dialog asks for another.
	keys["soniox"] = "wrong-key"
	saveVoiceFile(h.server.voicePath(), voiceFile{Keys: keys, Settings: &settings})
	messages = h.dictate("", pcm)
	if last := messages[len(messages)-1]; last["type"] != "error" || last["setup"] != true || !strings.Contains(fmt.Sprint(last["message"]), "Invalid API key") {
		t.Fatalf("a key refused on the stream: %v", messages)
	}
}

func TestYandexTakesLongTakesInPieces(t *testing.T) {
	var mu sync.Mutex
	var sizes []int
	srv := httptest.NewServer(http.HandlerFunc(func(w http.ResponseWriter, r *http.Request) {
		data, _ := io.ReadAll(r.Body)
		mu.Lock()
		sizes = append(sizes, len(data))
		mu.Unlock()
		// Each piece says which it is: its first sample.
		fmt.Fprintf(w, `{"result":"piece%d"}`, data[0])
	}))
	defer srv.Close()
	done := make(chan voiceHeard, 1)
	up, err := openYandex(t.Context(), voiceCall{API: srv.URL, Key: "k", Options: voiceOptions{Languages: []string{"en", "ru"}}, Heard: func(h voiceHeard) { done <- h }})
	if err != nil {
		t.Fatal(err)
	}
	defer up.close()
	pcm := make([]byte, yandexSpan*2+voiceRate*2)
	for i := range 3 {
		pcm[i*yandexSpan] = byte(i + 1)
	}
	up.audio(pcm)
	up.finish()
	heard := <-done
	if heard.Err != nil || heard.Final != "piece1 piece2 piece3" || !heard.Done {
		t.Fatalf("heard %+v", heard)
	}
	slices.Sort(sizes)
	if fmt.Sprint(sizes) != fmt.Sprint([]int{voiceRate * 2, yandexSpan, yandexSpan}) {
		t.Fatalf("pieces of %v bytes", sizes)
	}
}

func TestVoiceFewTerms(t *testing.T) {
	names := []string{"composer.js", "SessionScreen", "voice.go", "a-very-long-name-of-a-file.ts"}
	o := voiceOptions{Swearing: true, Words: []string{"Kou"}, Terms: append(append([]string{"Kou"}, voiceSwearing...), names...)}
	got := o.few(9, 20, "")
	// The user's words, then the swearing and the names in halves.
	if fmt.Sprint(got) != fmt.Sprint(append(append([]string{"Kou"}, voiceSwearing[:5]...), names[:3]...)) {
		t.Fatalf("few = %q", got)
	}
	// Without enough names, the swearing takes their room.
	if got := o.few(100, 0, ""); len(got) != min(100, len(o.Terms)) || !slices.Contains(got, "a-very-long-name-of-a-file.ts") {
		t.Fatalf("few(100) = %d %q", len(got), got)
	}
	if got := (voiceOptions{Terms: []string{"a<b>", "ok"}}).few(5, 0, "<>"); fmt.Sprint(got) != "[ok]" {
		t.Fatalf("few without <> = %q", got)
	}
}

func TestVoiceResampler(t *testing.T) {
	r := newVoiceResampler(voiceRate, openAIRate)
	pcm := speech(1)
	var out bytes.Buffer
	for i := 0; i < len(pcm); i += 3200 {
		out.Write(r.convert(pcm[i : i+3200]))
	}
	// Three samples for two, the tone as it was: 220 Hz at 24 kHz.
	if n := out.Len() / 2; n < openAIRate-2 || n > openAIRate {
		t.Fatalf("%d samples", n)
	}
	data := out.Bytes()
	for i := 0; i+1 < len(data) && i < 2000*2; i += 2 {
		got := float64(int16(uint16(data[i]) | uint16(data[i+1])<<8))
		want := 8000 * math.Sin(float64(i/2)*2*math.Pi*220/openAIRate)
		if math.Abs(got-want) > 40 {
			t.Fatalf("sample %d is %v, not %v", i/2, got, want)
		}
	}
}

func TestPolicyKeepsVoiceAtHome(t *testing.T) {
	h := newHarness(t)
	res, _ := h.do("GET", "/api/config", "")
	policy := res.Header.Get("Content-Security-Policy")
	host := strings.TrimPrefix(h.http.URL, "http://")
	if !strings.Contains(policy, "connect-src 'self' ws://"+host+" wss://"+host+";") {
		t.Fatalf("policy: %s", policy)
	}
}

func TestVoiceHints(t *testing.T) {
	for _, c := range []struct {
		languages []string
		want      string
	}{
		{nil, "[en]"},
		{[]string{"ru-RU", "en-RU"}, "[ru en]"},
		{[]string{"en-US", "en-GB"}, "[en]"},
		{[]string{"iw", "in-ID", "fil", "nn-NO", "zh_Hant_TW"}, "[he id tl no en]"},
		{[]string{"de", "fr", "es", "it", "pt"}, "[de fr es it en]"},
		{[]string{"tlh", "", "x-pirate"}, "[en]"},
	} {
		if got := fmt.Sprint(voiceHints(c.languages)); got != c.want {
			t.Errorf("voiceHints(%q) = %s, want %s", c.languages, got, c.want)
		}
	}
}

func TestVoiceTerms(t *testing.T) {
	root := t.TempDir()
	files := map[string]string{
		"src/deep/old.go": "", "src/new-file.ts": "", "README.md": "", "go/x.go": "",
		"node_modules/pkg/index.js": "", "src/main_test.go": "", ".github/ci.yml": "", "src/42.txt": "",
		// Assets, lockfiles and hashes are left out, however new.
		"Assets.xcassets/AppIcon.appiconset/Contents.json": "", "Assets.xcassets/AppIcon.appiconset/AppIcon.png": "",
		"src/Noto.ttf": "", "go.sum": "", "web/package-lock.json": "", "audit-234b16c9/notes.md": "", "src/0f3a9c1d2e.json": "",
	}
	writeFiles(t, root, files)
	now := time.Now()
	for name, age := range map[string]time.Duration{"src/deep/old.go": 3 * time.Hour, "README.md": 2 * time.Hour, "src/new-file.ts": time.Hour, "go/x.go": 4 * time.Hour} {
		if err := os.Chtimes(filepath.Join(root, name), now.Add(-age), now.Add(-age)); err != nil {
			t.Fatal(err)
		}
	}
	entries := []string{"src/", "src/deep/", "go/", "node_modules/", "node_modules/pkg/", ".github/"}
	for name := range files {
		entries = append(entries, name)
	}
	slices.Sort(entries)
	got := voiceTerms(root, entries)
	want := []string{filepath.Base(root), "src", "deep", "new-file.ts", "new-file", "README.md", "old.go", "x.go"}
	if resolved, err := filepath.EvalSymlinks(root); err == nil && filepath.Base(resolved) != filepath.Base(root) {
		want = slices.Insert(want, 1, filepath.Base(resolved))
	}
	if !slices.Equal(got, want) {
		t.Fatalf("voiceTerms = %q, want %q", got, want)
	}

	// Many names stop at the bounds, the folders' first.
	entries = entries[:0]
	for i := range 2000 {
		entries = append(entries, fmt.Sprintf("folder%04d/", i), fmt.Sprintf("folder%04d/FileNumber%04d.go", i, i))
	}
	got = voiceTerms(root, entries)
	size := 0
	for _, term := range got {
		size += len(term) + 1
	}
	if len(got) > maxVoiceTerms || size > maxVoiceBytes+1 || got[1] != "folder0000" || !strings.HasPrefix(got[maxVoiceFolders+1], "FileNumber") {
		t.Fatalf("%d terms of %d bytes: %q", len(got), size, got)
	}
}
