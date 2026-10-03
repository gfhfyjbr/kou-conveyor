package canvas

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"regexp"
	"slices"
	"strconv"
	"strings"
)

// Templates are canvases without what runs for them: nodes, their places
// and configuration, and edges, which make a new canvas's first batch. The
// canvas has its gallery (Pair, Fan-out, Issue triage, Empty); plugins add
// theirs (canvas.templates, a directory of *.json); a workspace keeps those
// saved from its canvases in .harness/canvas-templates.

// Template is a canvas to start from.
type Template struct {
	ID          string         `json:"id"`
	Title       string         `json:"title"`
	Description string         `json:"description,omitzero"`
	Source      string         `json:"source,omitzero"` // canvas, a plugin's name, or workspace
	Nodes       []TemplateNode `json:"nodes"`
	Edges       []TemplateEdge `json:"edges"`
}

// TemplateNode is a node of a template; Ref names it for the edges.
type TemplateNode struct {
	Ref    string         `json:"ref"`
	Kind   string         `json:"kind"`
	Preset string         `json:"preset,omitzero"`
	Plugin string         `json:"plugin,omitzero"`
	Title  string         `json:"title,omitzero"`
	X      int            `json:"x"`
	Y      int            `json:"y"`
	W      int            `json:"w,omitzero"`
	H      int            `json:"h,omitzero"`
	Config jsontext.Value `json:"config,omitzero"`
	Access string         `json:"access,omitzero"`
}

// TemplateEdge is an edge of a template, "ref:port" to "ref:port".
type TemplateEdge struct {
	From     string `json:"from"`
	To       string `json:"to"`
	Template string `json:"template,omitzero"`
	Mode     string `json:"mode,omitzero"`
	Deliver  string `json:"deliver,omitzero"`
}

var templateName = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9 ._-]{0,63}$`)

// gallery are the canvas's own templates.
var gallery = []Template{
	{
		ID: "pair", Title: "Pair", Source: canvasPlugin,
		Description: "Claude Code codes in a worktree; a kou agent reviews each turn, and its review goes back once you approve it.",
		Nodes: []TemplateNode{
			{Ref: "coder", Kind: KindTerminal, Preset: "claude-code", Title: "Coder", X: 0, Y: 0, Config: jsontext.Value(`{"worktree":{"name":"pair"}}`)},
			{Ref: "reviewer", Kind: KindAgent, Title: "Reviewer", X: 800, Y: 0, Config: jsontext.Value(`{"instructions":"Review the coder's work: read the diff in its worktree, point out bugs and gaps, and say LGTM when it is done."}`)},
		},
		Edges: []TemplateEdge{
			{From: "coder:out", To: "reviewer:in"},
			{From: "reviewer:out", To: "coder:in", Mode: ModeApprove},
		},
	},
	{
		ID: "fan-out", Title: "Fan-out", Source: canvasPlugin,
		Description: "A foreman splits the work among three shells' agents and gathers what they say.",
		Nodes: []TemplateNode{
			{Ref: "foreman", Kind: KindAgent, Preset: "foreman", Title: "Foreman", X: 0, Y: 0},
			{Ref: "a", Kind: KindTerminal, Preset: "shell", Title: "Worker A", X: 520, Y: -500},
			{Ref: "b", Kind: KindTerminal, Preset: "shell", Title: "Worker B", X: 520, Y: 0},
			{Ref: "c", Kind: KindTerminal, Preset: "shell", Title: "Worker C", X: 520, Y: 500},
		},
		Edges: []TemplateEdge{
			{From: "a:out", To: "foreman:in"},
			{From: "b:out", To: "foreman:in"},
			{From: "c:out", To: "foreman:in"},
		},
	},
	{
		ID: "issue-triage", Title: "Issue triage", Source: canvasPlugin,
		Description: "Events come to a triage agent, which decides what each needs and spawns the work.",
		Nodes: []TemplateNode{
			{Ref: "events", Kind: KindSource, Preset: "manual", Title: "Issues", X: 0, Y: 0, Config: jsontext.Value(`{"text":"Paste an issue here"}`)},
			{Ref: "triage", Kind: KindAgent, Title: "Triage", X: 400, Y: 0, Config: jsontext.Value(`{"instructions":"Triage each issue you are sent: find duplicates, label it, and for a real bug spawn a coder in a worktree with the issue as its prompt."}`)},
			{Ref: "notes", Kind: KindNote, Title: "How it works", X: 0, Y: 280, Config: jsontext.Value(`{"text":"Replace the Issues source with a plugin's (examples/plugins/github-events) to bring real issues."}`)},
		},
		Edges: []TemplateEdge{{From: "events:out", To: "triage:in"}},
	},
	{ID: "empty", Title: "Empty", Source: canvasPlugin, Description: "Nothing yet: add what you need.", Nodes: []TemplateNode{}, Edges: []TemplateEdge{}},
}

// templates lists the templates a workspace has.
func (e *Engine) templates(ws Workspace) []Template {
	out := append([]Template(nil), gallery...)
	for _, current := range e.o.Host.Plugins(ws).Plugins {
		if !current.Active || current.Canvas == nil || current.Canvas.Templates == "" {
			continue
		}
		sub, err := current.Sub(current.Canvas.Templates)
		if err != nil {
			continue
		}
		entries, _ := fs.ReadDir(sub, ".")
		for _, entry := range entries {
			name, ok := strings.CutSuffix(entry.Name(), ".json")
			if !ok || entry.IsDir() {
				continue
			}
			data, err := fs.ReadFile(sub, entry.Name())
			if err != nil {
				continue
			}
			var t Template
			if json.Unmarshal(data, &t) != nil {
				continue
			}
			t.ID, t.Source = current.Name+"/"+name, current.Name
			if t.Title == "" {
				t.Title = name
			}
			out = append(out, t)
		}
	}
	dir := filepath.Join(ws.Path, ".harness", "canvas-templates")
	entries, _ := os.ReadDir(dir)
	for _, entry := range entries {
		name, ok := strings.CutSuffix(entry.Name(), ".json")
		if !ok || entry.IsDir() {
			continue
		}
		data, err := os.ReadFile(filepath.Join(dir, entry.Name()))
		if err != nil {
			continue
		}
		var t Template
		if json.Unmarshal(data, &t) != nil {
			continue
		}
		t.ID, t.Source = "workspace/"+name, "workspace"
		if t.Title == "" {
			t.Title = name
		}
		out = append(out, t)
	}
	return out
}

// findTemplate finds a template by its ID.
func (e *Engine) findTemplate(ws Workspace, id string) (Template, bool) {
	for _, t := range e.templates(ws) {
		if t.ID == id {
			return t, true
		}
	}
	return Template{}, false
}

// ops makes the batch that builds a template on a canvas, moved by dx and
// dy.
func (t Template) ops(dx, dy int) []Op {
	var ops []Op
	refs := map[string]int{}
	for _, n := range t.Nodes {
		x, y := n.X+dx, n.Y+dy
		spec := &NodeSpec{Kind: n.Kind, Preset: n.Preset, Plugin: n.Plugin, Title: n.Title, X: &x, Y: &y, Exact: true, Config: n.Config, Access: n.Access}
		if n.W > 0 {
			w := n.W
			spec.W = &w
		}
		if n.H > 0 {
			h := n.H
			spec.H = &h
		}
		refs[n.Ref] = len(ops)
		ops = append(ops, Op{Op: "node.add", Node: spec})
	}
	for _, e := range t.Edges {
		fromRef, fromPort := splitPort(e.From, "out")
		toRef, toPort := splitPort(e.To, "in")
		from, okFrom := refs[fromRef]
		to, okTo := refs[toRef]
		if !okFrom || !okTo {
			continue
		}
		ops = append(ops, Op{Op: "edge.add", Edge: &EdgeSpec{
			From: Port{"$" + strconv.Itoa(from), fromPort}, To: Port{"$" + strconv.Itoa(to), toPort},
			Template: e.Template, Mode: e.Mode, Deliver: e.Deliver,
		}})
	}
	return ops
}

// saveTemplate saves a canvas as a template of its workspace.
func (c *Canvas) saveTemplate(name, description string) (Template, error) {
	name = strings.TrimSpace(name)
	if !templateName.MatchString(name) {
		return Template{}, errInvalid("a template's name is letters, digits, spaces, dots, dashes and underscores")
	}
	c.mu.Lock()
	t := Template{Title: name, Description: description, Nodes: []TemplateNode{}, Edges: []TemplateEdge{}}
	refs := map[string]string{}
	for i, n := range c.doc.Nodes {
		ref := "n" + strconv.Itoa(i+1)
		refs[n.ID] = ref
		t.Nodes = append(t.Nodes, TemplateNode{
			Ref: ref, Kind: n.Kind, Preset: n.Preset, Plugin: n.Plugin, Title: n.Title,
			X: n.X, Y: n.Y, W: n.W, H: n.H, Config: n.Config, Access: n.Access,
		})
	}
	for _, e := range c.doc.Edges {
		t.Edges = append(t.Edges, TemplateEdge{
			From: refs[e.From.Node] + ":" + e.From.Port, To: refs[e.To.Node] + ":" + e.To.Port,
			Template: e.Template, Mode: e.Mode, Deliver: e.Deliver,
		})
	}
	c.mu.Unlock()
	data, err := json.Marshal(t, jsontext.WithIndent("  "))
	if err != nil {
		return Template{}, err
	}
	file := strings.Map(func(r rune) rune {
		if r == ' ' {
			return '-'
		}
		return r
	}, strings.ToLower(name))
	path := filepath.Join(c.ws.Path, ".harness", "canvas-templates", file+".json")
	if err := writeAtomic(path, append(data, '\n'), 0o644); err != nil {
		return Template{}, fmt.Errorf("save the template: %w", err)
	}
	t.ID, t.Source = "workspace/"+file, "workspace"
	return t, nil
}

// templateSummaries are the templates without their nodes, for the
// gallery.
func templateSummaries(list []Template) []map[string]any {
	out := make([]map[string]any, 0, len(list))
	for _, t := range list {
		kinds := []string{}
		for _, n := range t.Nodes {
			name := n.Kind
			if n.Preset != "" {
				name = n.Preset
			}
			if !slices.Contains(kinds, name) {
				kinds = append(kinds, name)
			}
		}
		out = append(out, map[string]any{
			"id": t.ID, "title": t.Title, "description": t.Description, "source": t.Source,
			"nodes": len(t.Nodes), "edges": len(t.Edges), "kinds": kinds,
		})
	}
	return out
}
