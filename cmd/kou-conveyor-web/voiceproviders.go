package main

import (
	"bytes"
	"cmp"
	"context"
	"encoding/base64"
	endian "encoding/binary" // explorer.go has a binary
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"mime/multipart"
	"net/http"
	"net/url"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"time"
	"unicode/utf8"

	"github.com/gorilla/websocket"
)

// The speech-to-text providers dictation can listen with. Each takes the
// audio of a take as it comes, 16 kHz mono 16-bit little-endian PCM, and
// tells what it heard (voiceHeard): the live ones as it is said, the others
// once the take stops.

// voiceProvider is one provider: who it is, for the dialog, and how its key
// is checked and a stream opened.
type voiceProvider struct {
	ID, Name, Model string
	// Live says that words come as they are said.
	Live bool
	// Pieces says that what it hears comes in pieces of words, spaces
	// included, to be joined as they are; otherwise in words or phrases,
	// joined with spaces.
	Pieces bool
	// About is what it is, and is good at.
	About string
	// Console is where its keys are made.
	Console string
	// Env are the variables of the cockpit's environment its key may be in.
	Env []string
	// API is the address of its HTTP API, Socket that of its streams.
	API, Socket string
	// Wait is how long the last words may take once the take stops.
	Wait time.Duration
	// check says whether the provider takes a key: errVoiceRefused if not.
	check func(ctx context.Context, api, key string) error
	// open opens a stream.
	open func(ctx context.Context, call voiceCall) (voiceUpstream, error)
}

// voiceCall is what a stream opens with.
type voiceCall struct {
	API, Socket string // the provider's addresses
	Key         string
	Options     voiceOptions
	// Heard is told what the provider heard, from a goroutine of the
	// stream's.
	Heard func(voiceHeard)
}

// voiceOptions are how the provider is to listen.
type voiceOptions struct {
	// Languages are the codes of the languages spoken (voiceLanguages), the
	// main one first; there is one at least.
	Languages []string
	Strict    bool
	// Swearing says that Russian swearing is among Terms.
	Swearing bool
	// Coding says that the talk is instructions to a coding agent working
	// in Workspace.
	Coding    bool
	Workspace string
	// Terms are the words to hear and spell so, the most important first:
	// the user's Words, the swearing, the names of the workspace.
	Terms []string
	Words []string
}

// voiceHeard is what a provider heard.
type voiceHeard struct {
	// Final are words it is now sure of, after those it was sure of before.
	Final string
	// Partial is its guess at the words after those, instead of the last.
	Partial string
	// Done says that it is sure of all the audio it was sent.
	Done bool
	Err  error
}

// voiceUpstream is a stream to a provider.
type voiceUpstream interface {
	// audio sends audio, in the order heard.
	audio(pcm []byte) error
	// finish says that all the audio is sent: the provider is to be sure of
	// it all, and say Done.
	finish() error
	close()
}

const voiceRate = 16000 // Hz of the audio the providers are sent

// voiceProviders are the providers, in the order the first with a key is
// taken when none is chosen.
var voiceProviders = []*voiceProvider{
	{
		ID: "soniox", Name: "Soniox", Model: "stt-rt-v5", Live: true, Pieces: true,
		About:   "Words as they are said, the final ones fastest. Russian and English in one breath; told your words, Russian swearing and the workspace's names, 300 of them.",
		Console: "https://console.soniox.com", Env: []string{"SONIOX_API_KEY"},
		API: "https://api.soniox.com", Socket: "wss://stt-rt.soniox.com", Wait: 8 * time.Second,
		check: checkSoniox, open: openSoniox,
	},
	{
		ID: "deepgram", Name: "Deepgram", Model: "nova-3", Live: true,
		About:   "Nova-3, words as they are said. Russian and English mixed; told 100 of your words, swearing and names.",
		Console: "https://console.deepgram.com", Env: []string{"DEEPGRAM_API_KEY"},
		API: "https://api.deepgram.com", Socket: "wss://api.deepgram.com", Wait: 8 * time.Second,
		check: checkDeepgram, open: openDeepgram,
	},
	{
		ID: "elevenlabs", Name: "ElevenLabs", Model: "scribe_v2_realtime", Live: true,
		About:   "Scribe v2 Realtime, words as they are said; strong on Russian. Told 50 short words at most.",
		Console: "https://elevenlabs.io/app/settings/api-keys", Env: []string{"ELEVENLABS_API_KEY"},
		API: "https://api.elevenlabs.io", Socket: "wss://api.elevenlabs.io", Wait: 8 * time.Second,
		check: checkElevenLabs, open: openElevenLabs,
	},
	{
		ID: "openai", Name: "OpenAI", Model: "gpt-live-transcribe", Live: true,
		About:   "GPT Live Transcribe, words as they are said; told the topic and 100 of your words.",
		Console: "https://platform.openai.com/api-keys", Env: []string{"OPENAI_API_KEY"},
		API: "https://api.openai.com", Socket: "wss://api.openai.com", Wait: 10 * time.Second,
		check: checkOpenAI, open: openOpenAI,
	},
	{
		ID: "gemini", Name: "Google Gemini", Model: "gemini-3.5-transcribe-live", Live: true,
		About:   "Gemini 3.5 Transcribe Live, words as they are said; told 100 of your words.",
		Console: "https://aistudio.google.com/apikey", Env: []string{"GEMINI_API_KEY", "GOOGLE_API_KEY"},
		API: "https://generativelanguage.googleapis.com", Socket: "wss://generativelanguage.googleapis.com", Wait: 10 * time.Second,
		check: checkGemini, open: openGemini,
	},
	{
		ID: "groq", Name: "Groq", Model: "whisper-large-v3",
		About:   "Whisper large v3: the text comes all at once when you stop, in a blink. Cheap.",
		Console: "https://console.groq.com/keys", Env: []string{"GROQ_API_KEY"},
		API: "https://api.groq.com", Wait: 60 * time.Second,
		check: checkGroq, open: openGroq,
	},
	{
		ID: "yandex", Name: "Yandex SpeechKit", Model: "general",
		About:   "Russian first: the text comes when you stop. The API key of a service account with the ai.speechkit-stt.user role.",
		Console: "https://console.yandex.cloud", Env: []string{"YANDEX_API_KEY"},
		API: "https://stt.api.cloud.yandex.net", Wait: 60 * time.Second,
		check: checkYandex, open: openYandex,
	},
}

// voiceProviderOf is the provider of an ID, nil for none.
func voiceProviderOf(id string) *voiceProvider {
	for _, p := range voiceProviders {
		if p.ID == id {
			return p
		}
	}
	return nil
}

// addresses are a provider's, or those tests gave.
func (v *voiceState) addresses(p *voiceProvider) (api, socket string) {
	if base := v.bases[p.ID]; base != "" {
		return base, "ws" + strings.TrimPrefix(base, "http")
	}
	return p.API, p.Socket
}

// errVoiceRefused is a provider refusing a key.
var errVoiceRefused = errors.New("the key was refused")

// voiceRefusal is a provider refusing a key, with what it said.
type voiceRefusal struct{ name, message string }

func (e voiceRefusal) Error() string {
	return strings.TrimSuffix(e.name+" refused the key: "+e.message, ": ")
}

func (e voiceRefusal) Is(target error) bool { return target == errVoiceRefused }

// voiceMessage is what a provider's answer says went wrong, in any of the
// shapes they say it in.
func voiceMessage(body []byte) string {
	var answer struct {
		Message      string `json:"message"`
		ErrMsg       string `json:"err_msg"`
		ErrorMessage string `json:"error_message"`
		Error        any    `json:"error"`
		Detail       any    `json:"detail"`
	}
	if json.Unmarshal(body, &answer) == nil {
		nested := func(v any) string {
			switch v := v.(type) {
			case string:
				return v
			case map[string]any:
				if m, ok := v["message"].(string); ok {
					return m
				}
			}
			return ""
		}
		if m := cmp.Or(answer.Message, answer.ErrMsg, answer.ErrorMessage, nested(answer.Error), nested(answer.Detail)); m != "" {
			return m
		}
	}
	text := strings.Join(strings.Fields(string(body)), " ")
	if len(text) > 200 {
		text = text[:200] + "…"
	}
	return text
}

// voiceRequest makes a request of a provider's API and reads its answer:
// an error unless it succeeded, a refusal of the key for the statuses
// given.
func voiceRequest(ctx context.Context, name string, req *http.Request, refused ...int) ([]byte, error) {
	ctx, cancel := context.WithTimeout(ctx, 60*time.Second)
	defer cancel()
	res, err := http.DefaultClient.Do(req.WithContext(ctx))
	if err != nil {
		return nil, fmt.Errorf("%s could not be reached: %w", name, err)
	}
	defer res.Body.Close()
	body, _ := io.ReadAll(io.LimitReader(res.Body, 1<<20))
	if res.StatusCode/100 == 2 {
		return body, nil
	}
	message := cmp.Or(voiceMessage(body), http.StatusText(res.StatusCode))
	if slices.Contains(refused, res.StatusCode) {
		return nil, voiceRefusal{name, message}
	}
	return nil, fmt.Errorf("%s answered %d: %s", name, res.StatusCode, message)
}

// checkWith checks a key with a request that needs it.
func checkWith(ctx context.Context, name, method, address string, header http.Header, body []byte, refused ...int) error {
	req, err := http.NewRequestWithContext(ctx, method, address, bytes.NewReader(body))
	if err != nil {
		return err
	}
	req.Header = header
	_, err = voiceRequest(ctx, name, req, refused...)
	return err
}

// ---------------------------------------------------------------- streams

// voiceSocket is a provider's WebSocket.
type voiceSocket struct {
	name   string
	conn   *websocket.Conn
	mu     sync.Mutex // orders the writes
	closed atomic.Bool
}

// dialVoice opens a provider's WebSocket. Refused for its key, it is a
// refusal.
func dialVoice(ctx context.Context, name, address string, header http.Header) (*voiceSocket, error) {
	dialer := websocket.Dialer{HandshakeTimeout: 10 * time.Second, Proxy: http.ProxyFromEnvironment}
	conn, res, err := dialer.DialContext(ctx, address, header)
	if err != nil {
		if res != nil {
			body, _ := io.ReadAll(io.LimitReader(res.Body, 64<<10))
			message := cmp.Or(voiceMessage(body), http.StatusText(res.StatusCode))
			if res.StatusCode == http.StatusUnauthorized || res.StatusCode == http.StatusForbidden {
				return nil, voiceRefusal{name, message}
			}
			return nil, fmt.Errorf("%s refused the stream (%d): %s", name, res.StatusCode, message)
		}
		return nil, fmt.Errorf("%s could not be reached: %w", name, err)
	}
	conn.SetReadLimit(4 << 20)
	return &voiceSocket{name: name, conn: conn}, nil
}

func (s *voiceSocket) write(kind int, data []byte) error {
	s.mu.Lock()
	defer s.mu.Unlock()
	s.conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
	if err := s.conn.WriteMessage(kind, data); err != nil {
		return fmt.Errorf("%s: the stream broke: %w", s.name, err)
	}
	return nil
}

func (s *voiceSocket) send(v any) error {
	data, err := json.Marshal(v)
	if err != nil {
		return err
	}
	return s.write(websocket.TextMessage, data)
}

func (s *voiceSocket) close() {
	if s.closed.Swap(true) {
		return
	}
	s.mu.Lock()
	s.conn.WriteControl(websocket.CloseMessage, websocket.FormatCloseMessage(websocket.CloseNormalClosure, ""), time.Now().Add(time.Second))
	s.mu.Unlock()
	s.conn.Close()
}

// read calls each with the stream's messages, until it returns false or the
// stream ends; ended is told why it ended, unless it was closed here.
func (s *voiceSocket) read(each func([]byte) bool, ended func(error)) {
	go func() {
		for {
			_, data, err := s.conn.ReadMessage()
			if err != nil {
				if !s.closed.Load() {
					ended(err)
				}
				return
			}
			if !each(data) {
				return
			}
		}
	}()
}

// broken is a stream's end as an error: what the provider said in closing
// it, a refusal if that was of the key.
func (s *voiceSocket) broken(err error) error {
	var closed *websocket.CloseError
	if errors.As(err, &closed) {
		if strings.Contains(strings.ToLower(closed.Text), "api key") || strings.Contains(closed.Text, "invalid_api_key") {
			return voiceRefusal{s.name, closed.Text}
		}
		text := closed.Text
		if len(text) > 300 {
			text = text[:300] + "…"
		}
		if text != "" {
			text = ": " + text
		}
		return fmt.Errorf("%s closed the stream (%d%s)", s.name, closed.Code, text)
	}
	return fmt.Errorf("%s: the stream broke: %w", s.name, err)
}

// few are the terms for a provider that takes fewer than Terms: at most n,
// each of at most runes characters (0 for any) and without the characters
// in banned. The user's words come first, then as much of the swearing as
// of the names, so that neither crowds out the other.
func (o voiceOptions) few(n, runes int, banned string) []string {
	var words, slang, names []string
	for _, term := range o.Terms {
		if (runes > 0 && utf8.RuneCountInString(term) > runes) || strings.ContainsAny(term, banned) {
			continue
		}
		switch {
		case slices.Contains(o.Words, term):
			words = append(words, term)
		case o.Swearing && slices.Contains(voiceSwearing, term):
			slang = append(slang, term)
		default:
			names = append(names, term)
		}
	}
	out := slices.Clone(words[:min(n, len(words))])
	left := n - len(out)
	swearing := min(len(slang), max(left/2, left-len(names)))
	out = append(out, slang[:swearing]...)
	return append(out, names[:min(len(names), left-swearing)]...)
}

// fit are the few terms (see few) that fit in size bytes, each followed by
// a separator of sep bytes.
func (o voiceOptions) fit(n, size, sep int) []string {
	bytes := func(terms []string) (total int) {
		for _, term := range terms {
			total += len(term) + sep
		}
		return total
	}
	terms := o.few(n, 0, "")
	for len(terms) > 0 && bytes(terms) > size {
		terms = o.few(len(terms)-1, 0, "")
	}
	return terms
}

// voicePrompt tells a model that takes prose what the talk is.
func voicePrompt(o voiceOptions) string {
	var parts []string
	if o.Coding {
		parts = append(parts, "Dictated instructions to a coding agent working in "+o.Workspace+"; names of files and code are said in English.")
	}
	if o.Swearing {
		parts = append(parts, "Russian is spoken with swearing and slang: write them as said.")
	}
	return strings.Join(parts, " ")
}

// pcmWAV is PCM as a WAV file.
func pcmWAV(pcm []byte) []byte {
	le := endian.LittleEndian
	b := make([]byte, 0, 44+len(pcm))
	b = le.AppendUint32(append(b, "RIFF"...), uint32(36+len(pcm)))
	b = le.AppendUint32(append(b, "WAVEfmt "...), 16)
	b = le.AppendUint16(le.AppendUint16(b, 1), 1)                   // PCM, mono
	b = le.AppendUint32(le.AppendUint32(b, voiceRate), voiceRate*2) // rate, bytes a second
	b = le.AppendUint16(le.AppendUint16(b, 2), 16)                  // bytes a frame, bits a sample
	b = le.AppendUint32(append(b, "data"...), uint32(len(pcm)))
	return append(b, pcm...)
}

// ---------------------------------------------------------------- Soniox

func checkSoniox(ctx context.Context, api, key string) error {
	return checkWith(ctx, "Soniox", http.MethodGet, api+"/v1/models", http.Header{"Authorization": {"Bearer " + key}}, nil, 401, 403)
}

const sonioxModel = "stt-rt-v5"

// sonioxConfig is the first message of a Soniox stream.
func sonioxConfig(o voiceOptions) map[string]any {
	config := map[string]any{"model": sonioxModel, "audio_format": "pcm_s16le", "sample_rate": voiceRate, "num_channels": 1, "language_hints": o.Languages}
	if o.Strict {
		config["language_hints_strict"] = true
	}
	about := map[string]any{} // the stream's context
	if o.Coding {
		about["general"] = []map[string]string{
			{"key": "domain", "value": "Software development"},
			{"key": "topic", "value": "Instructions to a coding agent working in " + o.Workspace},
		}
	}
	if len(o.Terms) > 0 {
		about["terms"] = o.Terms
	}
	if len(about) > 0 {
		config["context"] = about
	}
	return config
}

type sonioxStream struct{ *voiceSocket }

func openSoniox(ctx context.Context, c voiceCall) (voiceUpstream, error) {
	s, err := dialVoice(ctx, "Soniox", c.Socket+"/transcribe-websocket", http.Header{"Authorization": {"Bearer " + c.Key}})
	if err != nil {
		return nil, err
	}
	if err := s.send(sonioxConfig(c.Options)); err != nil {
		s.close()
		return nil, err
	}
	s.read(func(data []byte) bool {
		var m struct {
			Tokens []struct {
				Text  string `json:"text"`
				Final bool   `json:"is_final"`
			} `json:"tokens"`
			Finished     bool   `json:"finished"`
			ErrorCode    any    `json:"error_code"`
			ErrorType    string `json:"error_type"`
			ErrorMessage string `json:"error_message"`
		}
		if json.Unmarshal(data, &m) != nil {
			return true
		}
		if m.ErrorCode != nil {
			message := cmp.Or(m.ErrorMessage, m.ErrorType, "the stream failed")
			if code := fmt.Sprint(m.ErrorCode); code == "401" || code == "403" {
				c.Heard(voiceHeard{Err: voiceRefusal{"Soniox", message}})
			} else {
				c.Heard(voiceHeard{Err: fmt.Errorf("Soniox: %s (%v)", message, m.ErrorCode)})
			}
			return false
		}
		var final, partial strings.Builder
		fin := false
		for _, token := range m.Tokens {
			switch {
			case token.Text == "<fin>":
				fin = true
			case token.Text == "<end>":
			case token.Final:
				final.WriteString(token.Text)
			default:
				partial.WriteString(token.Text)
			}
		}
		c.Heard(voiceHeard{Final: final.String(), Partial: partial.String(), Done: fin || m.Finished})
		return !m.Finished
	}, func(err error) { c.Heard(voiceHeard{Err: s.broken(err)}) })
	return sonioxStream{s}, nil
}

func (s sonioxStream) audio(pcm []byte) error { return s.write(websocket.BinaryMessage, pcm) }
func (s sonioxStream) finish() error          { return s.send(map[string]string{"type": "finalize"}) }

func (s sonioxStream) close() {
	if !s.closed.Load() {
		s.write(websocket.TextMessage, nil) // the end of the audio
	}
	s.voiceSocket.close()
}

// ---------------------------------------------------------------- Deepgram

func checkDeepgram(ctx context.Context, api, key string) error {
	// A key without the right to list projects still transcribes: only a
	// 401 is a refusal.
	err := checkWith(ctx, "Deepgram", http.MethodGet, api+"/v1/projects", http.Header{"Authorization": {"Token " + key}}, nil, 401)
	if err != nil && !errors.Is(err, errVoiceRefused) && strings.Contains(err.Error(), "answered 403") {
		return nil
	}
	return err
}

// deepgramMulti are the languages nova-3 hears mixed (language=multi).
var deepgramMulti = []string{"en", "es", "fr", "de", "hi", "ru", "pt", "ja", "it", "nl"}

// deepgramQuery is the query a Deepgram stream opens with.
func deepgramQuery(o voiceOptions) url.Values {
	q := url.Values{
		"model": {"nova-3"}, "encoding": {"linear16"}, "sample_rate": {fmt.Sprint(voiceRate)}, "channels": {"1"},
		"interim_results": {"true"}, "smart_format": {"true"}, "punctuate": {"true"},
	}
	language := o.Languages[0]
	if len(o.Languages) > 1 && !o.Strict && !slices.ContainsFunc(o.Languages, func(l string) bool { return !slices.Contains(deepgramMulti, l) }) {
		language = "multi"
	}
	q.Set("language", language)
	// The keyterms are 500 tokens at most, or the stream fails: some 1500
	// bytes.
	if terms := o.fit(100, 1500, 1); len(terms) > 0 {
		q["keyterm"] = terms
	}
	return q
}

type deepgramStream struct {
	*voiceSocket
	finishing atomic.Bool
}

func openDeepgram(ctx context.Context, c voiceCall) (voiceUpstream, error) {
	s, err := dialVoice(ctx, "Deepgram", c.Socket+"/v1/listen?"+deepgramQuery(c.Options).Encode(), http.Header{"Authorization": {"Token " + c.Key}})
	if err != nil {
		return nil, err
	}
	up := &deepgramStream{voiceSocket: s}
	s.read(func(data []byte) bool {
		var m struct {
			Type    string `json:"type"`
			Final   bool   `json:"is_final"`
			Channel struct {
				Alternatives []struct {
					Transcript string `json:"transcript"`
				} `json:"alternatives"`
			} `json:"channel"`
			Description string `json:"description"`
			Message     string `json:"message"`
		}
		if json.Unmarshal(data, &m) != nil {
			return true
		}
		switch m.Type {
		case "Results":
			text := ""
			if len(m.Channel.Alternatives) > 0 {
				text = m.Channel.Alternatives[0].Transcript
			}
			if m.Final {
				c.Heard(voiceHeard{Final: text})
			} else {
				c.Heard(voiceHeard{Partial: text})
			}
		case "Metadata":
			// It comes as the stream closes, after the last results.
			if up.finishing.Load() {
				c.Heard(voiceHeard{Done: true})
				return false
			}
		case "Error":
			c.Heard(voiceHeard{Err: fmt.Errorf("Deepgram: %s", cmp.Or(m.Description, m.Message, "the stream failed"))})
			return false
		}
		return true
	}, func(err error) {
		if up.finishing.Load() && websocket.IsCloseError(err, websocket.CloseNormalClosure) {
			c.Heard(voiceHeard{Done: true})
			return
		}
		c.Heard(voiceHeard{Err: s.broken(err)})
	})
	return up, nil
}

func (s *deepgramStream) audio(pcm []byte) error { return s.write(websocket.BinaryMessage, pcm) }

func (s *deepgramStream) finish() error {
	s.finishing.Store(true)
	return s.send(map[string]string{"type": "CloseStream"})
}

// ---------------------------------------------------------------- ElevenLabs

func checkElevenLabs(ctx context.Context, api, key string) error {
	return checkWith(ctx, "ElevenLabs", http.MethodPost, api+"/v1/single-use-token/realtime_scribe", http.Header{"xi-api-key": {key}}, nil, 401, 403)
}

// elevenLabsQuery is the query an ElevenLabs stream opens with: the main
// language, and 50 terms of 20 characters at most, its bounds.
func elevenLabsQuery(o voiceOptions) url.Values {
	q := url.Values{
		"model_id": {"scribe_v2_realtime"}, "audio_format": {fmt.Sprintf("pcm_%d", voiceRate)}, "commit_strategy": {"manual"},
		"language_code": {o.Languages[0]},
	}
	for _, term := range o.few(50, 20, "") {
		q.Add("keyterms", term)
	}
	return q
}

type elevenLabsStream struct {
	*voiceSocket
	finishing atomic.Bool
}

func openElevenLabs(ctx context.Context, c voiceCall) (voiceUpstream, error) {
	s, err := dialVoice(ctx, "ElevenLabs", c.Socket+"/v1/speech-to-text/realtime?"+elevenLabsQuery(c.Options).Encode(), http.Header{"xi-api-key": {c.Key}})
	if err != nil {
		return nil, err
	}
	up := &elevenLabsStream{voiceSocket: s}
	s.read(func(data []byte) bool {
		var m struct {
			Type  string `json:"message_type"`
			Text  string `json:"text"`
			Error string `json:"error"`
		}
		if json.Unmarshal(data, &m) != nil {
			return true
		}
		switch m.Type {
		case "partial_transcript":
			c.Heard(voiceHeard{Partial: m.Text})
		case "committed_transcript":
			c.Heard(voiceHeard{Final: m.Text, Done: up.finishing.Load()})
		case "session_started", "committed_transcript_with_timestamps":
		default:
			if m.Error == "" {
				return true
			}
			switch {
			case m.Type == "auth_error":
				c.Heard(voiceHeard{Err: voiceRefusal{"ElevenLabs", m.Error}})
			case up.finishing.Load() && (m.Type == "insufficient_audio_activity" || m.Type == "commit_throttled"):
				c.Heard(voiceHeard{Done: true}) // nothing was said
			default:
				c.Heard(voiceHeard{Err: fmt.Errorf("ElevenLabs: %s (%s)", m.Error, m.Type)})
			}
			return false
		}
		return true
	}, func(err error) { c.Heard(voiceHeard{Err: s.broken(err)}) })
	return up, nil
}

func (s *elevenLabsStream) chunk(pcm []byte, commit bool) error {
	return s.send(map[string]any{"message_type": "input_audio_chunk", "audio_base_64": base64.StdEncoding.EncodeToString(pcm), "commit": commit, "sample_rate": voiceRate})
}

func (s *elevenLabsStream) audio(pcm []byte) error { return s.chunk(pcm, false) }

// finish commits with a tenth of a second of silence.
func (s *elevenLabsStream) finish() error {
	s.finishing.Store(true)
	return s.chunk(make([]byte, voiceRate/10*2), true)
}

// ---------------------------------------------------------------- OpenAI

func checkOpenAI(ctx context.Context, api, key string) error {
	return checkWith(ctx, "OpenAI", http.MethodGet, api+"/v1/models", http.Header{"Authorization": {"Bearer " + key}}, nil, 401, 403)
}

const openAIRate = 24000 // Hz of the audio OpenAI takes

// openAISession is the session.update an OpenAI stream opens with.
func openAISession(o voiceOptions) map[string]any {
	transcription := map[string]any{"model": "gpt-live-transcribe", "languages": o.Languages}
	if prompt := voicePrompt(o); prompt != "" {
		transcription["prompt"] = prompt
	}
	if keywords := o.few(100, 0, "<>\r\n"); len(keywords) > 0 {
		transcription["keywords"] = keywords
	}
	return map[string]any{"type": "session.update", "session": map[string]any{
		"type": "transcription",
		"audio": map[string]any{"input": map[string]any{
			"format":         map[string]any{"type": "audio/pcm", "rate": openAIRate},
			"transcription":  transcription,
			"turn_detection": nil,
		}},
	}}
}

type openAIStream struct {
	*voiceSocket
	finishing atomic.Bool
	resampler *voiceResampler
}

func openOpenAI(ctx context.Context, c voiceCall) (voiceUpstream, error) {
	s, err := dialVoice(ctx, "OpenAI", c.Socket+"/v1/realtime?intent=transcription", http.Header{"Authorization": {"Bearer " + c.Key}})
	if err != nil {
		return nil, err
	}
	if err := s.send(openAISession(c.Options)); err != nil {
		s.close()
		return nil, err
	}
	up := &openAIStream{voiceSocket: s, resampler: newVoiceResampler(voiceRate, openAIRate)}
	var partial string // read by the reading goroutine alone
	s.read(func(data []byte) bool {
		var m struct {
			Type       string `json:"type"`
			Delta      string `json:"delta"`
			Transcript string `json:"transcript"`
			Error      *struct {
				Code    string `json:"code"`
				Message string `json:"message"`
			} `json:"error"`
		}
		if json.Unmarshal(data, &m) != nil {
			return true
		}
		switch m.Type {
		case "conversation.item.input_audio_transcription.delta":
			partial += m.Delta
			c.Heard(voiceHeard{Partial: partial})
		case "conversation.item.input_audio_transcription.completed":
			partial = ""
			c.Heard(voiceHeard{Final: m.Transcript, Done: up.finishing.Load()})
		case "conversation.item.input_audio_transcription.failed", "error":
			message, code := "the transcription failed", ""
			if m.Error != nil {
				message, code = cmp.Or(m.Error.Message, message), m.Error.Code
			}
			switch {
			case code == "invalid_api_key":
				c.Heard(voiceHeard{Err: voiceRefusal{"OpenAI", message}})
			case code == "input_audio_buffer_commit_empty" && up.finishing.Load():
				c.Heard(voiceHeard{Done: true}) // nothing was said
			default:
				c.Heard(voiceHeard{Err: fmt.Errorf("OpenAI: %s", message)})
			}
			return false
		}
		return true
	}, func(err error) { c.Heard(voiceHeard{Err: s.broken(err)}) })
	return up, nil
}

func (s *openAIStream) audio(pcm []byte) error {
	return s.send(map[string]string{"type": "input_audio_buffer.append", "audio": base64.StdEncoding.EncodeToString(s.resampler.convert(pcm))})
}

func (s *openAIStream) finish() error {
	s.finishing.Store(true)
	return s.send(map[string]string{"type": "input_audio_buffer.commit"})
}

// ---------------------------------------------------------------- Gemini

func checkGemini(ctx context.Context, api, key string) error {
	err := checkWith(ctx, "Google Gemini", http.MethodGet, api+"/v1beta/models?pageSize=1", http.Header{"x-goog-api-key": {key}}, nil, 401, 403)
	// An invalid key is a 400.
	if err != nil && strings.Contains(err.Error(), "answered 400") && strings.Contains(strings.ToLower(err.Error()), "api key") {
		return voiceRefusal{"Google Gemini", strings.TrimPrefix(err.Error(), "Google Gemini answered 400: ")}
	}
	return err
}

// geminiLanguages are the BCP 47 tags of the codes Gemini's live
// transcription knows.
var geminiLanguages = map[string]string{
	"ru": "ru-RU", "en": "en-US", "uk": "uk-UA", "de": "de-DE", "fr": "fr-FR", "es": "es-ES", "it": "it-IT", "pt": "pt-BR",
	"pl": "pl-PL", "nl": "nl-NL", "tr": "tr-TR", "ja": "ja-JP", "ko": "ko-KR", "hi": "hi-IN", "ar": "ar-EG", "zh": "cmn-CN",
}

// geminiSetup is the first message of a Gemini stream: transcription alone,
// its turns the takes'.
func geminiSetup(model string, o voiceOptions) map[string]any {
	transcription := map[string]any{}
	codes := []string{}
	for _, language := range o.Languages {
		if tag, ok := geminiLanguages[language]; ok {
			codes = append(codes, tag)
		}
	}
	if len(codes) > 0 {
		transcription["languageCodes"] = codes
	}
	if terms := o.few(100, 0, ""); len(terms) > 0 {
		transcription["customVocabulary"] = terms
	}
	return map[string]any{"setup": map[string]any{
		"model":                   "models/" + model,
		"generationConfig":        map[string]any{"responseModalities": []string{"TEXT"}},
		"realtimeInputConfig":     map[string]any{"automaticActivityDetection": map[string]any{"disabled": true}},
		"inputAudioTranscription": transcription,
	}}
}

type geminiStream struct {
	*voiceSocket
	finishing atomic.Bool
}

func openGemini(ctx context.Context, c voiceCall) (voiceUpstream, error) {
	address := c.Socket + "/ws/google.ai.generativelanguage.v1beta.GenerativeService.BidiGenerateContent"
	s, err := dialVoice(ctx, "Google Gemini", address, http.Header{"x-goog-api-key": {c.Key}})
	if err != nil {
		return nil, err
	}
	if err := s.send(geminiSetup("gemini-3.5-transcribe-live", c.Options)); err != nil {
		s.close()
		return nil, err
	}
	// Audio waits for the setup to be taken.
	s.conn.SetReadDeadline(time.Now().Add(10 * time.Second))
	for {
		_, data, err := s.conn.ReadMessage()
		if err != nil {
			s.close()
			return nil, s.broken(err)
		}
		var m struct {
			SetupComplete *struct{} `json:"setupComplete"`
		}
		if json.Unmarshal(data, &m) == nil && m.SetupComplete != nil {
			break
		}
	}
	s.conn.SetReadDeadline(time.Time{})
	if err := s.send(map[string]any{"realtimeInput": map[string]any{"activityStart": map[string]any{}}}); err != nil {
		s.close()
		return nil, err
	}
	up := &geminiStream{voiceSocket: s}
	s.read(func(data []byte) bool {
		var m struct {
			ServerContent *struct {
				Interim *struct {
					Text string `json:"text"`
				} `json:"interimInputTranscription"`
				Final *struct {
					Text string `json:"text"`
				} `json:"inputTranscription"`
				TurnComplete bool `json:"turnComplete"`
			} `json:"serverContent"`
		}
		if json.Unmarshal(data, &m) != nil || m.ServerContent == nil {
			return true
		}
		content := m.ServerContent
		switch {
		case content.Final != nil:
			c.Heard(voiceHeard{Final: content.Final.Text, Done: up.finishing.Load()})
		case content.Interim != nil:
			c.Heard(voiceHeard{Partial: content.Interim.Text})
		}
		if content.TurnComplete && up.finishing.Load() {
			c.Heard(voiceHeard{Done: true})
			return false
		}
		return true
	}, func(err error) { c.Heard(voiceHeard{Err: s.broken(err)}) })
	return up, nil
}

func (s *geminiStream) audio(pcm []byte) error {
	return s.send(map[string]any{"realtimeInput": map[string]any{"audio": map[string]string{
		"data": base64.StdEncoding.EncodeToString(pcm), "mimeType": fmt.Sprintf("audio/pcm;rate=%d", voiceRate),
	}}})
}

func (s *geminiStream) finish() error {
	s.finishing.Store(true)
	return s.send(map[string]any{"realtimeInput": map[string]any{"activityEnd": map[string]any{}}})
}

// ---------------------------------------------------------------- the others

// voiceBatch is a provider that hears a take once it stops: the audio is
// kept until then.
type voiceBatch struct {
	ctx        context.Context
	cancel     context.CancelFunc
	heard      func(voiceHeard)
	mu         sync.Mutex
	pcm        bytes.Buffer
	transcribe func(ctx context.Context, pcm []byte) (string, error)
}

// maxVoiceBatch bounds the audio kept: ten minutes.
const maxVoiceBatch = voiceRate * 2 * 600

func newVoiceBatch(ctx context.Context, c voiceCall, transcribe func(ctx context.Context, pcm []byte) (string, error)) *voiceBatch {
	ctx, cancel := context.WithCancel(ctx)
	return &voiceBatch{ctx: ctx, cancel: cancel, heard: c.Heard, transcribe: transcribe}
}

func (b *voiceBatch) audio(pcm []byte) error {
	b.mu.Lock()
	defer b.mu.Unlock()
	if b.pcm.Len()+len(pcm) > maxVoiceBatch {
		return errors.New("the take is longer than ten minutes, the most this provider is sent")
	}
	b.pcm.Write(pcm)
	return nil
}

func (b *voiceBatch) finish() error {
	b.mu.Lock()
	pcm := bytes.Clone(b.pcm.Bytes())
	b.mu.Unlock()
	go func() {
		// Less than a fifth of a second is nothing said.
		if len(pcm) < voiceRate*2/5 {
			b.heard(voiceHeard{Done: true})
			return
		}
		text, err := b.transcribe(b.ctx, pcm)
		if err != nil {
			b.heard(voiceHeard{Err: err})
			return
		}
		b.heard(voiceHeard{Final: strings.TrimSpace(text), Done: true})
	}()
	return nil
}

func (b *voiceBatch) close() { b.cancel() }

// Groq

func checkGroq(ctx context.Context, api, key string) error {
	return checkWith(ctx, "Groq", http.MethodGet, api+"/openai/v1/models", http.Header{"Authorization": {"Bearer " + key}}, nil, 401, 403)
}

// whisperPrompt is what Whisper is told before the audio: the talk, and
// the terms, as many as fit.
func whisperPrompt(o voiceOptions) string {
	// The prompt is 224 tokens at most: some 600 bytes, the words as many
	// as fit, the swearing and the names in halves.
	prompt := voicePrompt(o)
	terms := o.fit(60, 600-len(prompt), 2)
	if prompt != "" && len(terms) > 0 {
		prompt += " "
	}
	return prompt + strings.Join(terms, ", ")
}

func openGroq(ctx context.Context, c voiceCall) (voiceUpstream, error) {
	return newVoiceBatch(ctx, c, func(ctx context.Context, pcm []byte) (string, error) {
		var body bytes.Buffer
		form := multipart.NewWriter(&body)
		part, _ := form.CreateFormFile("file", "dictation.wav")
		part.Write(pcmWAV(pcm))
		form.WriteField("model", "whisper-large-v3")
		form.WriteField("response_format", "json")
		form.WriteField("temperature", "0")
		if len(c.Options.Languages) == 1 || c.Options.Strict {
			form.WriteField("language", c.Options.Languages[0])
		}
		if prompt := whisperPrompt(c.Options); prompt != "" {
			form.WriteField("prompt", prompt)
		}
		form.Close()
		req, err := http.NewRequestWithContext(ctx, http.MethodPost, c.API+"/openai/v1/audio/transcriptions", &body)
		if err != nil {
			return "", err
		}
		req.Header.Set("Authorization", "Bearer "+c.Key)
		req.Header.Set("Content-Type", form.FormDataContentType())
		data, err := voiceRequest(ctx, "Groq", req, 401, 403)
		if err != nil {
			return "", err
		}
		var answer struct {
			Text string `json:"text"`
		}
		if err := json.Unmarshal(data, &answer); err != nil {
			return "", fmt.Errorf("Groq's answer could not be read: %w", err)
		}
		return answer.Text, nil
	}), nil
}

// Yandex SpeechKit

// yandexLanguages are the tags of the codes SpeechKit knows.
var yandexLanguages = map[string]string{
	"ru": "ru-RU", "en": "en-US", "de": "de-DE", "uk": "uk-UA", "kk": "kk-KZ", "tr": "tr-TR", "fr": "fr-FR", "es": "es-ES",
	"it": "it-IT", "pl": "pl-PL", "pt": "pt-PT", "nl": "nl-NL", "fi": "fi-FI", "sv": "sv-SE", "he": "he-IL",
}

// yandexSpan is the most audio one request takes: 29 of its 30 seconds.
const yandexSpan = voiceRate * 2 * 29

// yandexRecognize recognises up to 30 seconds of audio.
func yandexRecognize(ctx context.Context, api, key, language string, pcm []byte) (string, error) {
	q := url.Values{"lang": {language}, "format": {"lpcm"}, "sampleRateHertz": {fmt.Sprint(voiceRate)}, "profanityFilter": {"false"}}
	req, err := http.NewRequestWithContext(ctx, http.MethodPost, api+"/speech/v1/stt:recognize?"+q.Encode(), bytes.NewReader(pcm))
	if err != nil {
		return "", err
	}
	req.Header.Set("Authorization", "Api-Key "+key)
	data, err := voiceRequest(ctx, "Yandex SpeechKit", req, 401, 403)
	if err != nil {
		return "", err
	}
	var answer struct {
		Result string `json:"result"`
	}
	if err := json.Unmarshal(data, &answer); err != nil {
		return "", fmt.Errorf("Yandex SpeechKit's answer could not be read: %w", err)
	}
	return answer.Result, nil
}

func checkYandex(ctx context.Context, api, key string) error {
	_, err := yandexRecognize(ctx, api, key, "ru-RU", make([]byte, voiceRate/5*2))
	return err
}

func openYandex(ctx context.Context, c voiceCall) (voiceUpstream, error) {
	language := "ru-RU"
	for _, code := range c.Options.Languages {
		if tag, ok := yandexLanguages[code]; ok {
			language = tag
			break
		}
	}
	return newVoiceBatch(ctx, c, func(ctx context.Context, pcm []byte) (string, error) {
		// Longer takes go in pieces of 29 seconds, at once.
		pieces := make([]string, (len(pcm)+yandexSpan-1)/yandexSpan)
		errs := make([]error, len(pieces))
		var wg sync.WaitGroup
		for i := range pieces {
			wg.Go(func() {
				pieces[i], errs[i] = yandexRecognize(ctx, c.API, c.Key, language, pcm[i*yandexSpan:min(len(pcm), (i+1)*yandexSpan)])
			})
		}
		wg.Wait()
		if err := errors.Join(errs...); err != nil {
			return "", errs[slices.IndexFunc(errs, func(err error) bool { return err != nil })]
		}
		return strings.Join(slices.DeleteFunc(pieces, func(s string) bool { return s == "" }), " "), nil
	}), nil
}

// ---------------------------------------------------------------- audio

// voiceResampler changes the rate of a stream of 16-bit PCM, linearly,
// chunk after chunk.
type voiceResampler struct {
	step float64 // input samples per output sample
	at   float64 // where the next output sample is, from the last input one kept
	last float64
	kept bool
}

func newVoiceResampler(from, to int) *voiceResampler {
	return &voiceResampler{step: float64(from) / float64(to)}
}

func (r *voiceResampler) convert(pcm []byte) []byte {
	in := make([]float64, 0, len(pcm)/2+1)
	if r.kept {
		in = append(in, r.last)
	}
	for i := 0; i+1 < len(pcm); i += 2 {
		in = append(in, float64(int16(endian.LittleEndian.Uint16(pcm[i:]))))
	}
	if len(in) < 2 {
		return nil
	}
	out := make([]byte, 0, int(float64(len(in))/r.step+2)*2)
	for r.at < float64(len(in)-1) {
		i := int(r.at)
		frac := r.at - float64(i)
		v := in[i]*(1-frac) + in[i+1]*frac
		out = endian.LittleEndian.AppendUint16(out, uint16(int16(max(-32768, min(32767, v)))))
		r.at += r.step
	}
	r.at -= float64(len(in) - 1)
	r.last, r.kept = in[len(in)-1], true
	return out
}
