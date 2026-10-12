package main

import (
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"os"
	"path"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"time"
	"unicode"
)

// Voice input: the composers, the page's and the iPhone's, stream what the
// microphone hears to the cockpit (voicesocket.go), which streams it on to
// the speech-to-text provider chosen in dictation's settings and sends the
// words back as they come (voiceproviders.go). The providers' keys are the
// cockpit's, the same in every workspace, and never leave it: a variable of
// the cockpit's environment (SONIOX_API_KEY, DEEPGRAM_API_KEY…), or else the
// key pasted in the page's dialog, kept in voice.json beside the settings
// (PUT /api/voice). The cockpit tells the provider of the languages the user
// speaks, and of the names of the workspace's folders and files, which the
// model then spells as they are spelled.

// voiceState is what voice input keeps between requests.
type voiceState struct {
	// bases are the providers' addresses by ID, for tests: their HTTP API's
	// and, with ws for http, their streams'.
	bases map[string]string

	// terms are the names of the workspaces' files the model is told of, by
	// workspace ID, kept for a while.
	termsMu sync.Mutex
	terms   map[string]voiceTermList

	// fileMu orders the changes of voice.json.
	fileMu sync.Mutex
}

type voiceTermList struct {
	terms []string
	at    time.Time     // when they were listed; zero for never
	ready chan struct{} // closed as the listing going on ends; nil for none
}

// voiceTermsWait is how long a take waits for the names of a workspace
// never listed: a large one takes seconds, and the take goes without them.
var voiceTermsWait = 300 * time.Millisecond

// Where a provider's key comes from.
const (
	voiceFromEnvironment = "environment" // a variable of the cockpit's
	voiceFromFile        = "file"        // voice.json
)

// voiceFile is voice.json: the keys pasted on the page, and how dictation
// listens.
type voiceFile struct {
	// Keys are the providers' keys by provider ID.
	Keys map[string]string `json:"keys,omitzero"`
	// Soniox is the key saved when Soniox was the only provider: Soniox's,
	// unless Keys has one.
	Soniox string `json:"soniox_api_key,omitzero"`
	// Settings are nil until they are saved once: the defaults hold.
	Settings *voiceSettings `json:"settings,omitzero"`
}

// key is the provider's saved key, "" for none.
func (f voiceFile) key(id string) string {
	if key := voiceKeyOf(f.Keys[id]); key != "" {
		return key
	}
	if id == "soniox" {
		return voiceKeyOf(f.Soniox)
	}
	return ""
}

// voiceSettings are how dictation listens, the same in every workspace and
// on the iPhone.
type voiceSettings struct {
	// Provider is the ID of the provider that listens: "" for the first,
	// in voiceProviders' order, that has a key.
	Provider string `json:"provider"`
	// Languages are the codes (ISO 639-1) of the languages spoken, the main
	// one first; none are the browser's or the phone's, and English.
	Languages []string `json:"languages"`
	// Strict keeps the model to those languages, where the provider can.
	Strict bool `json:"strict"`
	// Swearing tells the model of Russian swearing and slang (voiceSwearing)
	// when Russian is spoken: without it, fast or noisy speech with them
	// drifts into English words that sound alike.
	Swearing bool `json:"swearing"`
	// Files tells it of the names of the workspace's files and folders.
	Files bool `json:"files"`
	// Coding tells it that what is said is instructions to a coding agent.
	Coding bool `json:"coding"`
	// Words are the user's own, told before all others.
	Words []string `json:"words"`
}

func defaultVoiceSettings() voiceSettings {
	return voiceSettings{Languages: []string{}, Swearing: true, Files: true, Coding: true, Words: []string{}}
}

const (
	maxVoiceLanguages = 8
	// The user's words are told whole: they stay within the terms' bounds
	// (maxVoiceTerms, maxVoiceBytes), with room for others.
	maxVoiceWords     = 150
	maxVoiceWordBytes = 3000
)

// settings are the saved settings, or the defaults.
func (f voiceFile) settings() voiceSettings {
	if f.Settings == nil {
		return defaultVoiceSettings()
	}
	settings := *f.Settings
	if settings.Languages == nil {
		settings.Languages = []string{}
	}
	if settings.Words == nil {
		settings.Words = []string{}
	}
	return settings
}

// voicePath is where the keys saved from the page and the settings are
// kept, beside the cockpit's settings; "" when it has nowhere to keep them.
func (s *server) voicePath() string {
	if s.opt.SettingsFile == "" {
		return ""
	}
	return filepath.Join(filepath.Dir(s.opt.SettingsFile), "voice.json")
}

// readVoiceFile is voice.json as saved; none is empty.
func (s *server) readVoiceFile() (voiceFile, error) {
	var saved voiceFile
	file := s.voicePath()
	if file == "" {
		return saved, nil
	}
	data, err := os.ReadFile(file)
	if errors.Is(err, fs.ErrNotExist) {
		return saved, nil
	}
	if err == nil {
		err = json.Unmarshal(data, &saved)
	}
	if err != nil {
		return voiceFile{}, fmt.Errorf("%s: %w", file, err)
	}
	return saved, nil
}

// voiceKey is a provider's key and where it comes from: a variable of the
// cockpit's environment, or else voice.json; "" for none.
func (s *server) voiceKey(p *voiceProvider, saved voiceFile) (key, from string) {
	for _, name := range p.Env {
		if key := voiceKeyOf(os.Getenv(name)); key != "" {
			return key, voiceFromEnvironment
		}
	}
	if key := saved.key(p.ID); key != "" {
		return key, voiceFromFile
	}
	return "", ""
}

// voiceChoice is the provider that listens, its key and where that comes
// from: the one the settings name, or the first with a key; the key is ""
// when the one named has none, the provider nil when none is named and none
// has a key.
func (s *server) voiceChoice(saved voiceFile) (p *voiceProvider, key, from string) {
	if p := voiceProviderOf(saved.settings().Provider); p != nil {
		key, from := s.voiceKey(p, saved)
		return p, key, from
	}
	for _, p := range voiceProviders {
		if key, from := s.voiceKey(p, saved); key != "" {
			return p, key, from
		}
	}
	return nil, "", ""
}

// voiceReady says whether dictation can listen: its provider has a key.
func (s *server) voiceReady() bool {
	saved, _ := s.readVoiceFile()
	_, key, _ := s.voiceChoice(saved)
	return key != ""
}

// voiceKeyOf is a key as it was pasted or set, without the space and the
// quotes around it.
func voiceKeyOf(raw string) string {
	return strings.Trim(strings.TrimSpace(raw), `"'`)
}

// voiceHint is what the page sees of a key: its last characters.
func voiceHint(key string) string {
	if len(key) < 12 {
		return ""
	}
	return "…" + key[len(key)-4:]
}

// voiceView is what the page learns of dictation's setup: the providers,
// whether each has a key, from where, and its last characters, never the
// key; which one listens; and the settings.
type voiceView struct {
	// Set says that dictation can listen: Provider has a key.
	Set bool `json:"set"`
	// Provider is the ID of the provider that listens, "" when none is
	// chosen and none has a key.
	Provider  string              `json:"provider"`
	Providers []voiceProviderView `json:"providers"`
	// Path is where keys and settings saved from the page are kept, "" for
	// nowhere.
	Path     string        `json:"path,omitzero"`
	Settings voiceSettings `json:"settings"`
	// Languages are the codes a language may be given as.
	Languages []string `json:"languages"`
	// Problem is what is wrong with voice.json, which is then as if empty.
	Problem string `json:"problem,omitzero"`
}

type voiceProviderView struct {
	ID    string `json:"id"`
	Name  string `json:"name"`
	Model string `json:"model"`
	// Live says that words come as they are said; otherwise all of them
	// come once the take stops.
	Live    bool     `json:"live"`
	About   string   `json:"about"`
	Console string   `json:"console"`
	Env     []string `json:"env"`
	Set     bool     `json:"set"`
	From    string   `json:"from,omitzero"`
	Hint    string   `json:"hint,omitzero"`
}

func (s *server) voiceView() voiceView {
	saved, err := s.readVoiceFile()
	view := voiceView{Path: s.voicePath(), Settings: saved.settings(), Languages: voiceLanguages, Providers: []voiceProviderView{}}
	if err != nil {
		view.Problem = err.Error()
	}
	for _, p := range voiceProviders {
		key, from := s.voiceKey(p, saved)
		view.Providers = append(view.Providers, voiceProviderView{
			ID: p.ID, Name: p.Name, Model: p.Model, Live: p.Live, About: p.About, Console: p.Console, Env: p.Env,
			Set: key != "", From: from, Hint: voiceHint(key),
		})
	}
	if p, key, _ := s.voiceChoice(saved); p != nil {
		view.Provider, view.Set = p.ID, key != ""
	}
	return view
}

func (s *server) handleVoice(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, s.voiceView())
}

// handleSaveVoice keeps the keys pasted on the page, once their providers
// take them, and the settings; either may come alone.
func (s *server) handleSaveVoice(w http.ResponseWriter, r *http.Request) {
	var request struct {
		// Keys are keys by provider ID.
		Keys     map[string]string `json:"keys"`
		Settings *voiceSettings    `json:"settings"`
	}
	if !decodeJSON(w, r, &request) {
		return
	}
	if len(request.Keys) == 0 && request.Settings == nil {
		writeError(w, http.StatusBadRequest, "Nothing to save: send keys, settings or both")
		return
	}
	if s.voicePath() == "" {
		writeError(w, http.StatusConflict, "The cockpit has nowhere to keep dictation's settings: set KOU_CONVEYOR_CONFIG, or the providers' keys in its environment")
		return
	}
	var settings voiceSettings
	if request.Settings != nil {
		var err error
		if settings, err = normalVoiceSettings(*request.Settings); err != nil {
			writeError(w, http.StatusBadRequest, err.Error())
			return
		}
	}
	keys := make(map[string]string)
	for id, raw := range request.Keys {
		p := voiceProviderOf(id)
		if p == nil {
			writeError(w, http.StatusBadRequest, fmt.Sprintf("Dictation knows no provider %q", id))
			return
		}
		key := voiceKeyOf(raw)
		if _, from := s.voiceKey(p, voiceFile{}); from == voiceFromEnvironment {
			writeError(w, http.StatusConflict, fmt.Sprintf("The cockpit's environment sets %s, which wins over a saved key: change it there, or unset it", strings.Join(p.Env, " or ")))
			return
		}
		if key == "" || len(key) > 2048 || strings.ContainsFunc(key, func(r rune) bool { return unicode.IsSpace(r) || unicode.IsControl(r) }) {
			writeError(w, http.StatusBadRequest, "That is not an API key of "+p.Name)
			return
		}
		// A key the provider refuses is not kept.
		api, _ := s.voice.addresses(p)
		if err := p.check(r.Context(), api, key); err != nil {
			status := http.StatusBadGateway
			if errors.Is(err, errVoiceRefused) {
				status = http.StatusBadRequest
			}
			writeError(w, status, err.Error())
			return
		}
		keys[p.ID] = key
	}
	s.voice.fileMu.Lock()
	defer s.voice.fileMu.Unlock()
	// A broken voice.json is as if empty, and is replaced.
	saved, _ := s.readVoiceFile()
	saved.migrate()
	for id, key := range keys {
		saved.Keys[id] = key
	}
	if request.Settings != nil {
		saved.Settings = &settings
	}
	if err := saveVoiceFile(s.voicePath(), saved); err != nil {
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	writeJSON(w, http.StatusOK, s.voiceView())
}

// migrate moves the key of the days of Soniox alone among the others.
func (f *voiceFile) migrate() {
	if f.Keys == nil {
		f.Keys = make(map[string]string)
	}
	if key := voiceKeyOf(f.Soniox); key != "" && voiceKeyOf(f.Keys["soniox"]) == "" {
		f.Keys["soniox"] = key
	}
	f.Soniox = ""
}

// handleForgetVoice removes the saved key of a provider (?provider=); the
// settings stay.
func (s *server) handleForgetVoice(w http.ResponseWriter, r *http.Request) {
	p := voiceProviderOf(r.URL.Query().Get("provider"))
	if p == nil {
		writeError(w, http.StatusBadRequest, "Say whose key to remove: ?provider=")
		return
	}
	if file := s.voicePath(); file != "" {
		s.voice.fileMu.Lock()
		defer s.voice.fileMu.Unlock()
		saved, _ := s.readVoiceFile()
		saved.migrate()
		delete(saved.Keys, p.ID)
		var err error
		if len(saved.Keys) == 0 && saved.Settings == nil {
			if err = os.Remove(file); errors.Is(err, fs.ErrNotExist) {
				err = nil
			}
		} else {
			err = saveVoiceFile(file, saved)
		}
		if err != nil {
			writeError(w, http.StatusInternalServerError, err.Error())
			return
		}
	}
	writeJSON(w, http.StatusOK, s.voiceView())
}

// normalVoiceSettings are settings as kept: languages as their codes, words
// with their spaces collapsed, each once. An unknown provider or language,
// or too many languages or words, are errors.
func normalVoiceSettings(in voiceSettings) (voiceSettings, error) {
	out := in
	if out.Provider != "" && voiceProviderOf(out.Provider) == nil {
		return voiceSettings{}, fmt.Errorf("Dictation knows no provider %q", out.Provider)
	}
	out.Languages = []string{}
	for _, tag := range in.Languages {
		if strings.TrimSpace(tag) == "" {
			continue
		}
		code := voiceCode(tag)
		if !slices.Contains(voiceLanguages, code) {
			return voiceSettings{}, fmt.Errorf("Dictation does not know the language %q; it knows %s", strings.TrimSpace(tag), strings.Join(voiceLanguages, " "))
		}
		if !slices.Contains(out.Languages, code) {
			out.Languages = append(out.Languages, code)
		}
	}
	if len(out.Languages) > maxVoiceLanguages {
		return voiceSettings{}, fmt.Errorf("%d languages are too many: %d at most", len(out.Languages), maxVoiceLanguages)
	}
	out.Words = []string{}
	size := 0
	for _, word := range in.Words {
		word = strings.Join(strings.Fields(word), " ")
		if word == "" || slices.Contains(out.Words, word) {
			continue
		}
		out.Words = append(out.Words, word)
		size += len(word) + 1
	}
	if len(out.Words) > maxVoiceWords || size > maxVoiceWordBytes {
		return voiceSettings{}, fmt.Errorf("Your words are too many: %d at most, of %d bytes together", maxVoiceWords, maxVoiceWordBytes)
	}
	return out, nil
}

// saveVoiceFile writes voice.json whole, readable by the user alone.
func saveVoiceFile(file string, saved voiceFile) error {
	data, err := json.Marshal(saved)
	if err != nil {
		return err
	}
	if err := os.MkdirAll(filepath.Dir(file), 0o700); err != nil {
		return fmt.Errorf("save %s: %w", file, err)
	}
	temp, err := os.CreateTemp(filepath.Dir(file), ".voice-*.json") // 0600
	if err != nil {
		return fmt.Errorf("save %s: %w", file, err)
	}
	defer os.Remove(temp.Name())
	_, err = temp.Write(append(data, '\n'))
	if closeErr := temp.Close(); err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Rename(temp.Name(), file)
	}
	if err != nil {
		return fmt.Errorf("save %s: %w", file, err)
	}
	return nil
}

// voiceLanguages are the codes a language is given as: those the providers
// know, Soniox's list, which the others' mostly cover.
var voiceLanguages = strings.Fields(`af ar az be bg bn bs ca cs cy da de el en es et eu fa fi fr gl gu he hi
	hr hu id it ja kk kn ko lt lv mk ml mr ms nl no pa pl pt ro ru sk sl sq sr sv sw ta te th tl tr uk ur vi zh`)

// voiceCode is the code of a BCP 47 tag: its language, as ISO 639-1 names
// it (ru-RU is ru, nb is no).
func voiceCode(tag string) string {
	code, _, _ := strings.Cut(strings.ToLower(strings.TrimSpace(tag)), "-")
	code, _, _ = strings.Cut(code, "_")
	switch code {
	case "nb", "nn":
		return "no"
	case "iw":
		return "he"
	case "in":
		return "id"
	case "fil":
		return "tl"
	}
	return code
}

// voiceSwearing are Russian swearing and slang, told the model when Russian
// is spoken: on fast or noisy speech it otherwise hears English names of
// files in them ("заебал этот ебучий тест" came out "Enable STT test").
var voiceSwearing = strings.Fields(`бля блять блядь блин хуй хуя хуе хуйня хуево хуевый нахуй нахуя
	похуй нихуя дохуя охуеть охуенно охуел хуета пиздец пизда пиздато пиздеж пиздит спиздил распиздяй
	ебать ебал ебаный ебучий ебануть ебанутый заебал заебись наебнулось наебнулся уебище съебись отъебись
	проебал выебываться сука сучка мудак мудила долбоеб говно говнокод хер херня нахер похер нихера хрен
	нахрен жопа залупа фигня капец пипец офигеть охренеть`)

// voiceVocabulary are the terms the model is told of, the most important
// first, as long as they fit: the user's words, swearing when Russian is
// spoken, then the names of the workspace. Providers that take fewer take
// the first.
func voiceVocabulary(settings voiceSettings, hints, files []string) []string {
	terms, size := []string{}, 0
	seen := make(map[string]bool)
	add := func(list []string) {
		for _, term := range list {
			if len(terms) >= maxVoiceTerms {
				return
			}
			if seen[term] || size+len(term) > maxVoiceBytes {
				continue
			}
			seen[term] = true
			terms = append(terms, term)
			size += len(term) + 1
		}
	}
	add(settings.Words)
	if settings.Swearing && slices.Contains(hints, "ru") {
		add(voiceSwearing)
	}
	if settings.Files {
		add(files)
	}
	return terms
}

// voiceHints are the languages to expect: the first few of the user's that
// are known, and English, the language of the code.
func voiceHints(languages []string) []string {
	hints := []string{}
	for _, tag := range languages {
		code := voiceCode(tag)
		if len(hints) < 4 && slices.Contains(voiceLanguages, code) && !slices.Contains(hints, code) {
			hints = append(hints, code)
		}
	}
	if !slices.Contains(hints, "en") {
		hints = append(hints, "en")
	}
	return hints
}

// termsOf are the names of a workspace the model is told of. They are
// listed anew in the background once a minute old, a take going with those
// listed before meanwhile: it waits voiceTermsWait at most for the first.
func (v *voiceState) termsOf(ctx context.Context, ws *workspace) []string {
	v.termsMu.Lock()
	if v.terms == nil {
		v.terms = make(map[string]voiceTermList)
	}
	kept := v.terms[ws.ID]
	if !kept.at.IsZero() && time.Since(kept.at) < time.Minute {
		v.termsMu.Unlock()
		return kept.terms
	}
	if kept.ready == nil {
		kept.ready = make(chan struct{})
		v.terms[ws.ID] = kept
		go v.list(ws, kept.ready)
	}
	v.termsMu.Unlock()
	if !kept.at.IsZero() {
		return kept.terms
	}
	select {
	case <-kept.ready:
	case <-time.After(voiceTermsWait):
	case <-ctx.Done():
	}
	v.termsMu.Lock()
	defer v.termsMu.Unlock()
	return v.terms[ws.ID].terms
}

// list lists the names of a workspace, and closes ready.
func (v *voiceState) list(ws *workspace, ready chan struct{}) {
	defer close(ready)
	ctx, cancel := context.WithTimeout(context.Background(), time.Minute)
	defer cancel()
	entries, err := ws.files().Entries(ctx)
	terms := voiceTerms(ws.Path, entries)
	v.termsMu.Lock()
	defer v.termsMu.Unlock()
	kept := v.terms[ws.ID]
	if err == nil {
		kept.terms, kept.at = terms, time.Now()
	}
	kept.ready = nil
	v.terms[ws.ID] = kept
}

const (
	// The model spells the names it is told only while they are few and
	// the likely ones come first: the list stops at maxVoiceTerms names or
	// maxVoiceBytes, maxVoiceFolders of them folders'.
	maxVoiceTerms   = 300
	maxVoiceBytes   = 6000
	maxVoiceFolders = 80
	// maxVoiceStats bounds the files whose times are looked up.
	maxVoiceStats = 20_000
)

// voiceSkipped are folders whose names, and those of their files, nobody
// dictates.
var voiceSkipped = []string{"third_party", "testdata", "vendor", "node_modules"}

// voiceTerms are the names of a workspace, given its entries (folders end in
// a slash), most likely to be said first: the workspace's own, its folders',
// shallow ones first, and its files', those changed last first, with the
// stems of compound ones (SessionScreen of SessionScreen.swift).
func voiceTerms(root string, entries []string) []string {
	terms, size := []string{}, 0
	seen := make(map[string]bool)
	full := func() bool { return len(terms) >= maxVoiceTerms || size >= maxVoiceBytes }
	add := func(term string) bool {
		if seen[term] || size+len(term) > maxVoiceBytes || full() {
			return false
		}
		seen[term] = true
		terms = append(terms, term)
		size += len(term) + 1
		return true
	}
	add(filepath.Base(root))
	if resolved, err := filepath.EvalSymlinks(root); err == nil {
		add(filepath.Base(resolved))
	}
	var folders, files []string
	for _, entry := range entries {
		name, folder := strings.CutSuffix(entry, "/")
		switch {
		case !voiceWorthy(name):
		case folder:
			folders = append(folders, name)
		default:
			files = append(files, name)
		}
	}
	slices.SortStableFunc(folders, func(a, b string) int { return strings.Count(a, "/") - strings.Count(b, "/") })
	added := 0
	for _, folder := range folders {
		if added == maxVoiceFolders || full() {
			break
		}
		if add(path.Base(folder)) {
			added++
		}
	}
	type stamped struct {
		name string
		at   time.Time
	}
	recent := make([]stamped, 0, len(files))
	for i, name := range files {
		var at time.Time
		if i < maxVoiceStats {
			if info, err := os.Stat(filepath.Join(root, filepath.FromSlash(name))); err == nil {
				at = info.ModTime()
			}
		}
		recent = append(recent, stamped{name, at})
	}
	slices.SortStableFunc(recent, func(a, b stamped) int { return b.at.Compare(a.at) })
	for _, file := range recent {
		if full() {
			break
		}
		base := path.Base(file.name)
		add(base)
		if stem := strings.TrimSuffix(base, path.Ext(base)); stem != base && compound(stem) {
			add(stem)
		}
	}
	return terms
}

// voiceNoise are the extensions of files whose names nobody dictates, however
// often they change: images, fonts, sounds, archives, checksums and locks.
var voiceNoise = strings.Fields(`.png .jpg .jpeg .gif .ico .icns .webp .heic .bmp .tif .tiff
	.ttf .otf .woff .woff2 .eot .mp3 .wav .m4a .aac .ogg .mp4 .mov .pdf .zip .gz .tgz .tar .xz .7z
	.sum .lock .map`)

// voiceWorthy says whether the name of an entry is one to tell the model:
// not hidden, nor a test's, nor in a folder of others' code, of test data,
// of an Xcode asset catalog or named for a hash, nor an asset's or a
// lockfile's (voiceNoise), and a word rather than a number, a hash or a
// letter or two.
func voiceWorthy(name string) bool {
	for part := range strings.SplitSeq(name, "/") {
		if strings.HasPrefix(part, ".") || slices.Contains(voiceSkipped, part) || strings.HasSuffix(part, ".xcassets") || hashed(part) {
			return false
		}
	}
	base := path.Base(name)
	ext := path.Ext(base)
	stem := strings.TrimSuffix(base, ext)
	if slices.Contains(voiceNoise, strings.ToLower(ext)) || base == "package-lock.json" {
		return false
	}
	return len(base) >= 3 && len(base) <= 64 && !strings.HasSuffix(base, "_test.go") && strings.IndexFunc(stem, unicode.IsLetter) >= 0
}

// hashed says whether a name carries a hash or an ID: a part of six or more
// hexadecimal digits, one of them at least a digit (audit-234b16c9).
func hashed(name string) bool {
	for part := range strings.FieldsFuncSeq(name, func(r rune) bool { return r == '-' || r == '_' || r == '.' }) {
		if len(part) >= 6 && strings.IndexFunc(part, unicode.IsDigit) >= 0 && strings.Trim(strings.ToLower(part), "0123456789abcdef") == "" {
			return true
		}
	}
	return false
}

// compound says whether a name is made of words a speaker says apart:
// CamelCase, kebab-case or snake_case.
func compound(name string) bool {
	if len(name) < 3 || strings.IndexFunc(name, unicode.IsLetter) < 0 {
		return false
	}
	if strings.ContainsAny(name, "-_") {
		return true
	}
	runes := []rune(name)
	for i := 1; i < len(runes); i++ {
		if unicode.IsLower(runes[i-1]) && unicode.IsUpper(runes[i]) {
			return true
		}
	}
	return false
}
