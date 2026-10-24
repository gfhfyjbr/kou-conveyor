package main

import (
	"cmp"
	"context"
	"log"
	"os"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// titleTimeout bounds how long a model may take to title a session.
const titleTimeout = time.Minute

// autoTitle has a model title a session after its first prompt, while the
// prompt runs (cockpit.GenerateTitle): the session is shown under the title
// from then on, unless the user gives it one of their own. A session the
// user or a canvas titled keeps its title; the preferences may turn titles
// off, and name the model that makes them, else the prompt's makes it.
func (s *server) autoTitle(ws *workspace, current *run, prompt string) {
	dir, id := ws.opt.SessionDir, current.sessionID
	meta := cockpit.LoadMeta(dir, id)
	// A title made of a first prompt that this one replaces, rewound or
	// never delivered, no longer says what the session is about.
	if meta.AutoTitle != "" {
		if err := cockpit.SetAutoTitle(dir, id, ""); err != nil {
			log.Printf("session %s: drop the title: %v", id, err)
		}
	}
	// A prompt of pictures alone leaves the session untitled, as it is.
	prefs := cockpit.LoadPreferences(cockpit.PreferencesPath(s.opt.SettingsFile))
	if meta.Title != "" || meta.Canvas != "" || prefs.NoAutoTitles || strings.TrimSpace(prompt) == "" {
		return
	}
	model := cmp.Or(prefs.TitleModel, current.model)
	go func() {
		ctx, cancel := context.WithTimeout(s.ctx, titleTimeout)
		defer cancel()
		title, err := cockpit.GenerateTitle(ctx, ws.opt, model, prompt)
		if err != nil {
			log.Printf("session %s: no title from %s: %v", id, cmp.Or(model, "the connection's model"), err)
			return
		}
		// A session deleted meanwhile keeps nothing: deleting waits for its run.
		if _, err := os.Stat(cockpit.SessionPath(dir, id)); err != nil && !s.sessionRunning(ws, id) {
			return
		}
		if err := cockpit.SetAutoTitle(dir, id, title); err != nil {
			log.Printf("session %s: save the title: %v", id, err)
			return
		}
		// Those who follow the run show the title at once; the session list
		// has it from now on, and a run over by now leaves it to the list.
		current.publish(event{Type: "title", Text: title})
	}()
}
