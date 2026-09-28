package main

import (
	"encoding/json/v2"
	"errors"
	"io"
	"io/fs"
	"mime"
	"net/http"
	"strconv"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// Files in prompts. A prompt links a file or a folder by a $ reference in
// its text (cockpit.FileLinks), and the runner reads what the model sees of
// it as the prompt runs. The routes below serve the composer, which
// completes a $ to the workspace's files and shows what each reference
// links, and the transcript, which shows what the model saw of the files a
// prompt linked.

// maxLinkedText bounds the text whose links a composer asks for.
const maxLinkedText = 1 << 20

// handleFiles lists what a $ reference that has ?q= so far can complete
// to: the workspace's files and folders, the best first, or what is in a
// folder from / or ~/.
func (s *server) handleFiles(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	limit := 50
	if n, err := strconv.Atoi(r.URL.Query().Get("limit")); err == nil && n > 0 {
		limit = min(n, 200)
	}
	matches, err := ws.files().Complete(r.Context(), r.URL.Query().Get("q"), limit)
	if err != nil && len(matches) == 0 {
		writeError(w, http.StatusInternalServerError, "cannot list the workspace's files: "+err.Error())
		return
	}
	if matches == nil {
		matches = []cockpit.FileMatch{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"files": matches})
}

// linkView is what a reference of the composer's text links.
type linkView struct {
	Label     string `json:"label"`
	Path      string `json:"path"`
	Directory bool   `json:"directory,omitzero"`
	Size      int64  `json:"size,omitzero"`
	StartLine int    `json:"start_line,omitzero"`
	EndLine   int    `json:"end_line,omitzero"`
}

// handleLinks says what the $ references of a text link, {"text": …}: the
// files and folders the prompt would bring.
func (s *server) handleLinks(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	if media, _, _ := mime.ParseMediaType(r.Header.Get("Content-Type")); media != "application/json" {
		writeError(w, http.StatusUnsupportedMediaType, "expected application/json")
		return
	}
	var req struct {
		Text string `json:"text"`
	}
	if err := json.UnmarshalRead(io.LimitReader(r.Body, maxLinkedText), &req); err != nil {
		writeError(w, http.StatusBadRequest, "invalid request: "+err.Error())
		return
	}
	links := []linkView{}
	for _, link := range cockpit.FileLinks(req.Text, ws.Path) {
		links = append(links, linkView{
			Label: link.Label, Path: link.Path, Directory: link.Directory, Size: link.Size,
			StartLine: link.StartLine, EndLine: link.EndLine,
		})
	}
	writeJSON(w, http.StatusOK, map[string]any{"links": links})
}

// handlePromptFiles returns the files a prompt of the session linked, with
// the lines the model saw of each, as the session keeps them.
func (s *server) handlePromptFiles(w http.ResponseWriter, r *http.Request) {
	ws := s.workspaceOf(w, r)
	if ws == nil {
		return
	}
	id, message := r.PathValue("id"), r.PathValue("message")
	if _, err := uuid.Parse(message); !cockpit.ValidSessionID(id) || err != nil {
		writeError(w, http.StatusNotFound, "prompt not found")
		return
	}
	files, err := cockpit.PromptFiles(ws.opt.SessionDir, id, message)
	switch {
	case errors.Is(err, fs.ErrNotExist):
		writeError(w, http.StatusNotFound, "prompt not found")
		return
	case err != nil:
		writeError(w, http.StatusInternalServerError, err.Error())
		return
	}
	if files == nil {
		files = []cockpit.LinkedFile{}
	}
	writeJSON(w, http.StatusOK, map[string]any{"files": files})
}
