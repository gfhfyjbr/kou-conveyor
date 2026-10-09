package main

import (
	"errors"
	"fmt"
	"io/fs"
	"net/http"
	"regexp"
	"slices"
	"strconv"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Images of prompts. A browser sends a prompt's images with it, each with
// the label its text refers to it by; the server makes each one a model
// takes (cockpit.PrepareImage) and keeps those the text still names. The
// transcript describes a prompt's images, and the routes below serve their
// bytes: from the session file for a prompt the runner recorded, from the
// queue for one that waits. The pictures the agent looked at come from the
// session file too.

// maxPromptBytes bounds the body of a request that brings a prompt: its
// images, base64 in JSON, make it megabytes.
const maxPromptBytes = 128 << 20

// imageLabel is the form labels take: they reach the model and terminals.
var imageLabel = regexp.MustCompile(`^\[Image [1-9][0-9]{0,5}\]$`)

// servedTypes are the only media types images are served as.
var servedTypes = []string{"image/png", "image/jpeg", "image/gif", "image/webp"}

// prepareImages makes the images a prompt's text refers to ones a model
// takes.
func prepareImages(text string, images []cockpit.Image) ([]cockpit.Image, error) {
	if len(images) > 4*cockpit.MaxImages {
		return nil, fmt.Errorf("a prompt takes at most %d images", cockpit.MaxImages)
	}
	for _, image := range images {
		if !imageLabel.MatchString(image.Label) {
			return nil, fmt.Errorf("image label %q is not of the form [Image 1]", image.Label)
		}
	}
	referenced := cockpit.Referenced(text, images)
	prepared := make([]cockpit.Image, 0, len(referenced))
	for _, image := range referenced {
		img, err := cockpit.PrepareImage(image.Data)
		if err != nil {
			return nil, fmt.Errorf("%s: %w", image.Label, err)
		}
		img.Label = image.Label
		prepared = append(prepared, img)
	}
	return prepared, nil
}

// handlePromptImage serves image n of a prompt the session recorded. Its
// bytes never change: a prompt that is edited runs under a new ID.
func (s *server) handlePromptImage(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id, message := r.PathValue("id"), r.PathValue("message")
	n, err := strconv.Atoi(r.PathValue("n"))
	if _, uuidErr := uuid.Parse(message); !cockpit.ValidSessionID(id) || uuidErr != nil || err != nil || n < 0 {
		writeError(w, http.StatusNotFound, "image not found")
		return
	}
	images, err := cockpit.PromptImages(ws.opt.SessionDir, id, message)
	switch {
	case errors.Is(err, fs.ErrNotExist) || err == nil && n >= len(images):
		writeError(w, http.StatusNotFound, "image not found")
		return
	case err != nil:
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	serveImage(w, images[n], "private, max-age=31536000, immutable")
}

// handleToolImage serves the picture a ViewImage call of the session read,
// as the model got it. It never changes once the call has read it.
func (s *server) handleToolImage(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id, call := r.PathValue("id"), r.PathValue("call")
	if !cockpit.ValidSessionID(id) || !cockpit.ValidCallID(call) {
		writeError(w, http.StatusNotFound, "image not found")
		return
	}
	image, err := cockpit.ToolImage(ws.opt.SessionDir, id, call)
	switch {
	case errors.Is(err, fs.ErrNotExist):
		writeError(w, http.StatusNotFound, "image not found")
		return
	case err != nil:
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	serveImage(w, image, "private, max-age=31536000, immutable")
}

// handleQueuedImage serves image n of a prompt that waits in the queue.
func (s *server) handleQueuedImage(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id := r.PathValue("id")
	n, err := strconv.Atoi(r.PathValue("n"))
	if !cockpit.ValidSessionID(id) || err != nil || n < 0 {
		writeError(w, http.StatusNotFound, "image not found")
		return
	}
	s.mu.Lock()
	var image *cockpit.Image
	if q := s.queues[activeKey(ws, id)]; q != nil {
		if item, _, ok := q.Get(r.PathValue("item")); ok && n < len(item.Images) {
			image = &item.Images[n]
		}
	}
	s.mu.Unlock()
	if image == nil {
		writeError(w, http.StatusNotFound, "image not found")
		return
	}
	serveImage(w, *image, "private, no-cache")
}

func serveImage(w http.ResponseWriter, image cockpit.Image, cache string) {
	if !slices.Contains(servedTypes, image.MediaType) {
		writeError(w, http.StatusNotFound, "image not found")
		return
	}
	h := w.Header()
	h.Set("Content-Type", image.MediaType)
	h.Set("Content-Length", strconv.Itoa(len(image.Data)))
	h.Set("Cache-Control", cache)
	w.Write(image.Data)
}
