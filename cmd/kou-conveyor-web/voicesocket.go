package main

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"net/http"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"sync/atomic"
	"time"

	"github.com/gorilla/websocket"
)

// A composer dictates through a WebSocket of the cockpit's,
// GET /api/w/{ws}/voice/socket?languages=ru-RU,en&provider=, one take a
// socket. It sends the microphone's audio as binary frames, 16 kHz mono
// 16-bit little-endian PCM, and {"type":"finish"} when the take stops; the
// cockpit streams the audio on to the provider (voiceproviders.go), the one
// the settings choose unless ?provider= names another, and answers
//
//	{"type":"ready","provider":"soniox","name":"Soniox","live":true}  the provider listens
//	{"type":"text","final":"…","partial":"…"}  all it is sure of, and its guess at what follows
//	{"type":"done"}                             it is sure of the whole take
//	{"type":"error","message":"…","setup":true} the take failed; setup: a key is wanted
//
// then closes it. The text is final then partial, as they are. Audio sent
// before the provider listens waits for it. What goes wrong is told on the
// socket, never as a status: the iPhone takes a 401 for its pairing gone,
// and a page cannot read the body of a refused upgrade.

// maxVoiceQueue bounds the audio waiting for the provider: a minute of it.
const maxVoiceQueue = voiceRate * 2 * 60

// voiceTake is a take: the page's or the phone's socket, and what the
// provider heard.
type voiceTake struct {
	conn *websocket.Conn
	p    *voiceProvider
	from string // where the provider's key came from

	mu        sync.Mutex // orders the writes, and guards what follows
	queue     [][]byte   // audio for the provider
	queued    int
	finishing bool
	final     string
	partial   string
	sent      [2]string // the text last sent

	wake   chan struct{} // there is audio, or the take stops
	over   chan struct{} // closed as the take ends: done, or failed
	ended  atomic.Bool
	pumped chan struct{} // closed once the provider's stream is closed
}

func (s *server) handleVoiceSocket(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	conn, err := terminalUpgrader.Upgrade(w, r, nil)
	if err != nil {
		return // the upgrader answered
	}
	defer conn.Close()
	conn.SetReadLimit(1 << 20)
	ctx, cancel := context.WithCancel(r.Context())
	defer cancel()
	t := &voiceTake{conn: conn, wake: make(chan struct{}, 1), over: make(chan struct{}), pumped: make(chan struct{})}

	saved, _ := s.readVoiceFile()
	settings := saved.settings()
	p, key, from := s.voiceChoice(saved)
	if id := r.URL.Query().Get("provider"); id != "" {
		if p = voiceProviderOf(id); p == nil {
			t.fail(fmt.Errorf("Dictation knows no provider %q", id))
			t.drain()
			return
		}
		key, from = s.voiceKey(p, saved)
	}
	t.p, t.from = p, from
	if key == "" {
		t.fail(nil) // a key is wanted
		t.drain()
		return
	}

	hints := settings.Languages
	if len(hints) == 0 {
		hints = voiceHints(strings.Split(r.URL.Query().Get("languages"), ","))
	}
	go t.pump(ctx, func() (voiceUpstream, error) {
		var files []string
		if settings.Files {
			files = s.voice.termsOf(ctx, ws)
		}
		options := voiceOptions{
			Languages: hints, Strict: settings.Strict, Swearing: settings.Swearing && slices.Contains(hints, "ru"),
			Coding: settings.Coding, Workspace: filepath.Base(ws.Path), Terms: voiceVocabulary(settings, hints, files), Words: settings.Words,
		}
		api, socket := s.voice.addresses(p)
		return p.open(ctx, voiceCall{API: api, Socket: socket, Key: key, Options: options, Heard: t.heard})
	})
	go t.ping(ctx)
	t.read()
	cancel()
	<-t.pumped
}

// read takes the page's audio and its finish, until it goes, or is gone a
// while after the take ended.
func (t *voiceTake) read() {
	alive := func() {
		if !t.ended.Load() {
			t.conn.SetReadDeadline(time.Now().Add(terminalIdle))
		}
	}
	alive()
	t.conn.SetPongHandler(func(string) error { alive(); return nil })
	for {
		kind, data, err := t.conn.ReadMessage()
		if err != nil {
			return
		}
		alive()
		switch kind {
		case websocket.BinaryMessage:
			t.audio(data)
		case websocket.TextMessage:
			var message struct {
				Type string `json:"type"`
			}
			if json.Unmarshal(data, &message) == nil && message.Type == "finish" {
				t.mu.Lock()
				t.finishing = true
				t.mu.Unlock()
				t.poke()
			}
		}
	}
}

// drain reads what the page still sends, so that it reads the end of the
// take before the socket closes.
func (t *voiceTake) drain() {
	for {
		if _, _, err := t.conn.ReadMessage(); err != nil {
			return
		}
	}
}

func (t *voiceTake) audio(pcm []byte) {
	t.mu.Lock()
	if t.ended.Load() || t.finishing || len(pcm) == 0 {
		t.mu.Unlock()
		return
	}
	if t.queued+len(pcm) > maxVoiceQueue {
		t.mu.Unlock()
		t.fail(fmt.Errorf("%s does not keep up: a minute of audio waits for it", t.p.Name))
		return
	}
	t.queue = append(t.queue, pcm)
	t.queued += len(pcm)
	t.mu.Unlock()
	t.poke()
}

func (t *voiceTake) poke() {
	select {
	case t.wake <- struct{}{}:
	default:
	}
}

// pump opens the provider's stream, sends it the audio as it comes, then
// tells it the take stopped; it closes the stream once the take is over.
func (t *voiceTake) pump(ctx context.Context, open func() (voiceUpstream, error)) {
	defer close(t.pumped)
	up, err := open()
	if err != nil {
		t.fail(err)
		return
	}
	defer up.close()
	t.mu.Lock()
	t.send(map[string]any{"type": "ready", "provider": t.p.ID, "name": t.p.Name, "live": t.p.Live})
	t.mu.Unlock()
	// broke fails the take for a write the provider's stream refused, unless
	// the provider says why first.
	broke := func(err error) {
		select {
		case <-t.over:
		case <-time.After(time.Second):
			t.fail(err)
		}
	}
	finished := false
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.over:
			return
		case <-t.wake:
		}
		t.mu.Lock()
		queue, finishing := t.queue, t.finishing
		t.queue, t.queued = nil, 0
		t.mu.Unlock()
		for _, pcm := range queue {
			if err := up.audio(pcm); err != nil {
				broke(err)
				return
			}
		}
		if finishing && !finished {
			finished = true
			if err := up.finish(); err != nil {
				broke(err)
				return
			}
			late := time.AfterFunc(t.p.Wait, t.late)
			defer late.Stop()
		}
	}
}

// heard passes on what the provider heard.
func (t *voiceTake) heard(h voiceHeard) {
	if h.Err != nil {
		t.fail(h.Err)
		return
	}
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.ended.Load() {
		return
	}
	if h.Final != "" {
		t.final = t.join(t.final, h.Final)
	}
	t.partial = h.Partial
	t.text()
	if h.Done {
		t.send(map[string]string{"type": "done"})
		t.end()
	}
}

// late ends a take whose provider is not sure of it all in time with what
// it heard: its guesses as if it were sure of them.
func (t *voiceTake) late() {
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.ended.Load() {
		return
	}
	t.final, t.partial = t.join(t.final, t.partial), ""
	t.text()
	t.send(map[string]string{"type": "done"})
	t.end()
}

// join adds words to those before them: as they are, for a provider that
// sends pieces of words with their spaces, else after a space.
func (t *voiceTake) join(before, words string) string {
	if t.p.Pieces {
		return before + words
	}
	before, words = strings.TrimSpace(before), strings.TrimSpace(words)
	if before == "" || words == "" {
		return before + words
	}
	return before + " " + words
}

// text sends the text, if it changed.
func (t *voiceTake) text() {
	partial := t.partial
	if !t.p.Pieces {
		partial = strings.TrimSpace(partial)
		if partial != "" && strings.TrimSpace(t.final) != "" {
			partial = " " + partial
		}
	}
	if t.sent == [2]string{t.final, partial} {
		return
	}
	t.sent = [2]string{t.final, partial}
	t.send(map[string]string{"type": "text", "final": t.final, "partial": partial})
}

// fail ends the take with what went wrong: nil for the provider's key
// missing.
func (t *voiceTake) fail(err error) {
	t.mu.Lock()
	defer t.mu.Unlock()
	if t.ended.Load() {
		return
	}
	message, setup := t.explain(err)
	t.send(map[string]any{"type": "error", "message": message, "setup": setup})
	t.end()
}

// explain is what went wrong, as the user is told, and whether a key is to
// be pasted in the dialog.
func (t *voiceTake) explain(err error) (string, bool) {
	switch {
	case err == nil && t.p == nil:
		return "Dictation needs the key of a speech-to-text provider: choose one in the dialog of the microphone button and paste its key there, or set its variable for the cockpit (SONIOX_API_KEY, DEEPGRAM_API_KEY…).", true
	case err == nil:
		return fmt.Sprintf("Dictation needs a key of %s: paste it in the dialog of the microphone button, or set %s for the cockpit.", t.p.Name, strings.Join(t.p.Env, " or ")), true
	case errors.Is(err, errVoiceRefused) && t.from == voiceFromEnvironment:
		return err.Error() + ". Check " + strings.Join(t.p.Env, " or ") + " of the cockpit's environment.", false
	case errors.Is(err, errVoiceRefused):
		return err.Error() + ". Paste another in the dialog of the microphone button.", true
	}
	return err.Error(), false
}

// send writes a message to the page, unless the take ended; t.mu is held.
func (t *voiceTake) send(v any) {
	if t.ended.Load() {
		return
	}
	data, err := json.Marshal(v)
	if err != nil {
		return
	}
	t.conn.SetWriteDeadline(time.Now().Add(10 * time.Second))
	if t.conn.WriteMessage(websocket.TextMessage, data) != nil {
		t.end()
	}
}

// end ends the take: the socket closes once the page says it read the end,
// or in a few seconds; t.mu is held.
func (t *voiceTake) end() {
	if t.ended.Swap(true) {
		return
	}
	close(t.over)
	t.conn.WriteControl(websocket.CloseMessage, websocket.FormatCloseMessage(websocket.CloseNormalClosure, ""), time.Now().Add(time.Second))
	t.conn.SetReadDeadline(time.Now().Add(5 * time.Second))
}

// ping keeps the socket of a long take open through the proxies on the way.
func (t *voiceTake) ping(ctx context.Context) {
	ticker := time.NewTicker(terminalPing)
	defer ticker.Stop()
	for {
		select {
		case <-ctx.Done():
			return
		case <-t.over:
			return
		case <-ticker.C:
			if t.conn.WriteControl(websocket.PingMessage, nil, time.Now().Add(10*time.Second)) != nil {
				return
			}
		}
	}
}
