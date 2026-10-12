package main

import (
	"bytes"
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// The server keeps the folder sessions beside its settings: in it, a link
// to the sessions folder of each workspace, named as the API names the
// workspace (joroweave-ab893e), and index.json, which lists the sessions of
// them all. The sessions of every workspace are found there in one place —
// by the user in Finder, by an agent with one read — wherever the
// workspaces are. The links are for finding sessions, not for keeping them:
// backups do not follow links.
//
// The server owns the links it made, which .managed.json lists, and those
// it found already made as it would have made them. It leaves anything else
// in the folder alone: links of the user's, folders, files.
const (
	sessionIndexFile = "index.json"
	managedLinksFile = ".managed.json"
	// sessionIndexEvery is how often the index is made again when nothing
	// tells it to: sizes and times of sessions that run in other
	// front-ends, titles renamed.
	sessionIndexEvery = 15 * time.Second
	// sessionStatesEvery is how often what the server runs is looked at:
	// a run that starts or ends, a workspace added or removed, makes the
	// index again at once — but not more often than this.
	sessionStatesEvery = 2 * time.Second
)

// linkTarget is a folder of sessions the links folder links to.
type linkTarget struct {
	Name string // the link's name: the ID of the workspace
	Dir  string // the sessions folder
}

// managedLinks is .managed.json: the links the server made, by name, with
// what they link to.
type managedLinks struct {
	Links map[string]string `json:"links"`
}

// syncLinks makes dir hold a link to the folder of each target that exists,
// replaces the links it made that link elsewhere, and removes those it made
// to folders no longer listed or gone. A folder reached two ways — another
// case of its path on a file system that ignores case, a link — is linked
// once, under the first target's name. It returns the targets linked.
func syncLinks(dir string, targets []linkTarget) ([]linkTarget, error) {
	if err := os.MkdirAll(dir, 0o700); err != nil {
		return nil, fmt.Errorf("session links: %w", err)
	}
	var managed managedLinks
	if data, err := os.ReadFile(filepath.Join(dir, managedLinksFile)); err == nil {
		_ = json.Unmarshal(data, &managed)
	}
	if managed.Links == nil {
		managed.Links = map[string]string{}
	}
	before := len(managed.Links)
	changed := false

	var wanted []linkTarget
	var folders []fs.FileInfo
	names := map[string]bool{}
	for _, target := range targets {
		info, err := os.Stat(target.Dir)
		if err != nil || !info.IsDir() || names[target.Name] || !validLinkName(target.Name) {
			continue
		}
		if slices.ContainsFunc(folders, func(other fs.FileInfo) bool { return os.SameFile(other, info) }) {
			continue
		}
		names[target.Name] = true
		folders = append(folders, info)
		wanted = append(wanted, target)
	}

	var problems []error
	var linked []linkTarget
	for _, target := range wanted {
		link := filepath.Join(dir, target.Name)
		info, err := os.Lstat(link)
		switch {
		case errors.Is(err, fs.ErrNotExist):
			if err := replaceLink(link, target.Dir); err != nil {
				problems = append(problems, err)
				continue
			}
		case err != nil:
			problems = append(problems, err)
			continue
		case info.Mode()&fs.ModeSymlink == 0:
			// A folder or a file of the user's, under the name the link
			// would have.
			if _, ours := managed.Links[target.Name]; ours {
				delete(managed.Links, target.Name)
				changed = true
			}
			continue
		default:
			current, _ := os.Readlink(link)
			if current == target.Dir || sameFolder(link, target.Dir) {
				// Made by the server, before or by an earlier one: taken
				// over as it is.
				if managed.Links[target.Name] != current {
					managed.Links[target.Name] = current
					changed = true
				}
				linked = append(linked, target)
				continue
			}
			if _, ours := managed.Links[target.Name]; !ours {
				continue // the user's own link, to somewhere else
			}
			if err := replaceLink(link, target.Dir); err != nil {
				problems = append(problems, err)
				continue
			}
		}
		managed.Links[target.Name] = target.Dir
		changed = true
		linked = append(linked, target)
	}

	for name, made := range managed.Links {
		if names[name] {
			continue
		}
		link := filepath.Join(dir, name)
		info, err := os.Lstat(link)
		if err == nil && info.Mode()&fs.ModeSymlink != 0 {
			// Only the link as it was made: one the user changed since is
			// theirs now.
			if current, _ := os.Readlink(link); current == made {
				if err := os.Remove(link); err != nil && !errors.Is(err, fs.ErrNotExist) {
					problems = append(problems, err)
					continue
				}
			}
		} else if err != nil && !errors.Is(err, fs.ErrNotExist) {
			problems = append(problems, err)
			continue
		}
		delete(managed.Links, name)
		changed = true
	}

	if changed || len(managed.Links) != before {
		data, err := json.Marshal(managed, json.Deterministic(true))
		if err == nil {
			err = writeFileAtomic(filepath.Join(dir, managedLinksFile), append(data, '\n'))
		}
		if err != nil {
			problems = append(problems, err)
		}
	}
	return linked, errors.Join(problems...)
}

// validLinkName keeps a link inside the links folder and out of the way of
// its own files.
func validLinkName(name string) bool {
	return name != "" && !strings.HasPrefix(name, ".") && name != sessionIndexFile && filepath.Base(name) == name
}

// sameFolder reports whether link leads to the folder dir.
func sameFolder(link, dir string) bool {
	a, err := os.Stat(link)
	if err != nil {
		return false
	}
	b, err := os.Stat(dir)
	return err == nil && os.SameFile(a, b)
}

// replaceLink makes link link to target in one step: a new link beside it,
// renamed over it.
func replaceLink(link, target string) error {
	temporary := fmt.Sprintf("%s.%d.new", filepath.Join(filepath.Dir(link), "."+filepath.Base(link)), time.Now().UnixNano())
	if err := os.Symlink(target, temporary); err != nil {
		return fmt.Errorf("session links: %w", err)
	}
	if err := os.Rename(temporary, link); err != nil {
		_ = os.Remove(temporary)
		return fmt.Errorf("session links: %w", err)
	}
	return nil
}

// writeFileAtomic replaces path with data in one step, so that a reader
// sees the old file or the new one, never part of one.
func writeFileAtomic(path string, data []byte) error {
	temp, err := os.CreateTemp(filepath.Dir(path), "."+filepath.Base(path)+"-*")
	if err != nil {
		return err
	}
	defer os.Remove(temp.Name())
	_, err = temp.Write(data)
	if closeErr := temp.Close(); err == nil {
		err = closeErr
	}
	if err == nil {
		err = os.Chmod(temp.Name(), 0o600)
	}
	if err == nil {
		err = os.Rename(temp.Name(), path)
	}
	return err
}

// worktrees lists the other working trees of the git repository at path,
// as git keeps them: .git/worktrees/<name>/gitdir names each one's .git.
func worktrees(path string) []string {
	base := filepath.Join(path, ".git", "worktrees")
	entries, err := os.ReadDir(base)
	if err != nil {
		return nil
	}
	var found []string
	for _, entry := range entries {
		data, err := os.ReadFile(filepath.Join(base, entry.Name(), "gitdir"))
		if err != nil {
			continue
		}
		gitdir := filepath.Clean(strings.TrimSpace(string(data)))
		if filepath.IsAbs(gitdir) && filepath.Base(gitdir) == ".git" {
			found = append(found, filepath.Dir(gitdir))
		}
	}
	slices.Sort(found)
	return found
}

// sessionFolder is a folder of sessions the index lists: a workspace's, or
// that of a git worktree of a workspace, which has sessions when an agent
// ran in it on its own.
type sessionFolder struct {
	ID         string `json:"id"`
	Path       string `json:"path"`     // the workspace or worktree
	Sessions   string `json:"sessions"` // its sessions folder
	Link       string `json:"link,omitempty"`
	WorktreeOf string `json:"worktree_of,omitempty"`
	Missing    bool   `json:"missing,omitempty"`
	ws         *workspace
}

// indexedSession is a session as index.json lists it.
type indexedSession struct {
	ID         string    `json:"id"`
	Workspace  string    `json:"workspace"` // the ID of its folder in workspaces
	File       string    `json:"file"`
	Title      string    `json:"title"`
	Canvas     string    `json:"canvas,omitempty"`
	Node       string    `json:"node,omitempty"`
	UpdatedAt  time.Time `json:"updated_at"`
	PromptedAt time.Time `json:"prompted_at"`
	Size       int64     `json:"size"`
	// State is running, stopping or compacting for a run of this server;
	// busy for one of another front-end (the terminal cockpit, a runner
	// started by hand); idle otherwise.
	State  string `json:"state"`
	RunID  string `json:"run_id,omitempty"`
	Queued int    `json:"queued,omitempty"`
}

// sessionIndex is index.json.
type sessionIndex struct {
	About      string                    `json:"about"`
	UpdatedAt  time.Time                 `json:"updated_at"`
	Workspaces []sessionFolder           `json:"workspaces"`
	Sessions   map[string]indexedSession `json:"sessions"` // by ID
}

const sessionIndexAbout = "Every session of every workspace of the kou-conveyor web cockpit, by ID. " +
	"Each workspace's sessions folder is linked here under the workspace's ID. " +
	"The cockpit makes this file again as sessions change: do not edit it. The links are for finding sessions; " +
	"backups do not follow them."

// sessionFolders are the folders of sessions to link and index: the
// workspaces', then those of their git worktrees that have sessions.
func (s *server) sessionFolders() []sessionFolder {
	list := s.workspaces.all()
	var folders []sessionFolder
	for _, ws := range list {
		_, err := os.Stat(ws.Path)
		folders = append(folders, sessionFolder{ID: ws.ID, Path: ws.Path, Sessions: ws.opt.SessionDir, Missing: err != nil, ws: ws})
	}
	for _, ws := range list {
		for _, tree := range worktrees(ws.Path) {
			sessions := filepath.Join(tree, ".harness", "sessions")
			if info, err := os.Stat(sessions); err != nil || !info.IsDir() {
				continue
			}
			listed := slices.ContainsFunc(folders, func(other sessionFolder) bool {
				return other.Path == tree || sameFolder(other.Path, tree)
			})
			if !listed {
				folders = append(folders, sessionFolder{ID: workspaceID(tree), Path: tree, Sessions: sessions, WorktreeOf: ws.ID})
			}
		}
	}
	return folders
}

// sessionStates is what the server runs, as a key that changes when that
// does: the workspaces listed, the runs, whether they stop, the prompts
// queued.
func (s *server) sessionStates() string {
	var key strings.Builder
	for _, ws := range s.workspaces.all() {
		key.WriteString(ws.ID + "\n")
	}
	s.mu.Lock()
	runs := make(map[string]*run, len(s.active))
	var lines []string
	for name, current := range s.active {
		if current != nil {
			runs[name] = current
		}
	}
	for name, q := range s.queues {
		if q != nil && len(q.Items) != 0 {
			lines = append(lines, fmt.Sprintf("%s queued %d", name, len(q.Items)))
		}
	}
	s.mu.Unlock()
	for name, current := range runs {
		current.mu.Lock()
		stopping := current.stopping
		current.mu.Unlock()
		lines = append(lines, fmt.Sprintf("%s %s %v", name, current.id, stopping))
	}
	slices.Sort(lines)
	key.WriteString(strings.Join(lines, "\n"))
	return key.String()
}

// indexSessions lists the sessions of folders by ID. A session found in two
// folders is listed as the first has it.
func (s *server) indexSessions(folders []sessionFolder) map[string]indexedSession {
	sessions := map[string]indexedSession{}
	for _, folder := range folders {
		list, err := cockpit.ListSessions(folder.Sessions)
		if err != nil {
			continue
		}
		for _, info := range list {
			if _, listed := sessions[info.ID]; listed {
				continue
			}
			entry := indexedSession{
				ID: info.ID, Workspace: folder.ID, File: cockpit.SessionPath(folder.Sessions, info.ID), Title: info.Title,
				Canvas: info.Canvas, Node: info.Node, UpdatedAt: info.UpdatedAt, PromptedAt: info.PromptedAt, Size: info.Size,
				State: "idle",
			}
			if folder.ws != nil {
				s.mu.Lock()
				current := s.attachable(folder.ws, info.ID)
				if q := s.queues[activeKey(folder.ws, info.ID)]; q != nil {
					entry.Queued = len(q.Items)
				}
				s.mu.Unlock()
				if current != nil {
					current.mu.Lock()
					entry.RunID, entry.State = current.id, "running"
					switch {
					case current.stopping:
						entry.State = "stopping"
					case current.compact:
						entry.State = "compacting"
					}
					current.mu.Unlock()
				}
			}
			if entry.State == "idle" && cockpit.SessionBusy(folder.Sessions, info.ID) {
				entry.State = "busy"
			}
			sessions[info.ID] = entry
		}
	}
	return sessions
}

// sessionLinksDir is the links folder beside the settings file; none
// without one.
func sessionLinksDir(settings string) string {
	if settings == "" {
		return ""
	}
	return filepath.Join(filepath.Dir(settings), "sessions")
}

// sessionLinks keeps the links folder (see above) while the server runs.
type sessionLinks struct {
	dir  string
	last []byte // the index as last written, without its time
}

// keepSessionLinks links and indexes the sessions now, then again as what
// the server runs changes, and every sessionIndexEvery.
func (s *server) keepSessionLinks(ctx context.Context, links *sessionLinks) {
	states := time.NewTicker(sessionStatesEvery)
	defer states.Stop()
	var known string
	var made time.Time
	for {
		current := s.sessionStates()
		if current != known || time.Since(made) >= sessionIndexEvery {
			known, made = current, time.Now()
			if err := links.refresh(s); err != nil {
				fmt.Fprintln(os.Stderr, "kou-conveyor-web:", err)
			}
		}
		select {
		case <-ctx.Done():
			return
		case <-states.C:
		}
	}
}

// refresh links the folders of sessions and writes the index, unless it
// lists what it listed before.
func (l *sessionLinks) refresh(s *server) error {
	folders := s.sessionFolders()
	targets := make([]linkTarget, len(folders))
	for i, folder := range folders {
		targets[i] = linkTarget{Name: folder.ID, Dir: folder.Sessions}
	}
	linked, problem := syncLinks(l.dir, targets)
	for i := range folders {
		if slices.ContainsFunc(linked, func(target linkTarget) bool { return target.Name == folders[i].ID }) {
			folders[i].Link = folders[i].ID
		}
	}
	index := sessionIndex{About: sessionIndexAbout, Workspaces: folders, Sessions: s.indexSessions(folders)}
	content, err := json.Marshal(index, json.Deterministic(true))
	if err != nil {
		return errors.Join(problem, err)
	}
	if bytes.Equal(content, l.last) {
		return problem
	}
	index.UpdatedAt = time.Now().UTC()
	data, err := json.Marshal(index, json.Deterministic(true))
	if err == nil {
		err = writeFileAtomic(filepath.Join(l.dir, sessionIndexFile), append(data, '\n'))
	}
	if err != nil {
		return errors.Join(problem, fmt.Errorf("session index: %w", err))
	}
	l.last = content
	return problem
}
