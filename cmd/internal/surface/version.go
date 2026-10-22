package surface

import (
	"bufio"
	"errors"
	"io"
	"os"
	"path"
	"regexp"
	"strconv"
	"strings"
)

// ToolName is the name of the plugin's tool.
const ToolName = "UI"

// Marker is the first line of the result of a call that showed a surface:
// "UI deps v1 (.harness/ui/<session>/deps/v1.json)".
var Marker = regexp.MustCompile(`^UI ([a-z0-9][a-z0-9_-]*) v(\d+)( deleted)? \((\.harness/ui/[A-Za-z0-9_./-]+?\.json)\)`)

var (
	sessionPart = regexp.MustCompile(`^[A-Za-z0-9][A-Za-z0-9_.-]*$`)
	surfaceID   = regexp.MustCompile(`^[a-z0-9][a-z0-9_-]*$`)
	versionFile = regexp.MustCompile(`^v[1-9][0-9]{0,8}\.json$`)
)

// Call is what a call of the tool showed: the surface, the version it made
// and where it keeps it, and from its text the title and warnings.
type Call struct {
	Surface  string
	Version  int
	Deleted  bool
	Path     string // .harness/ui/<session>/<surface>/v<N>.json
	Session  string // the folder of the version: the session that made it
	Title    string
	Warnings []string
	Text     string // the surface as text, as the agent got it
}

// IsTool says whether a tool's name is the plugin's: "UI", or as a gateway
// shows it to the model, "<word>_UI" or "mcp__<server>__<word>_UI".
func IsTool(name string) bool {
	return name == ToolName || strings.HasSuffix(name, "_"+ToolName)
}

// ParseCall reads the result of a call of the tool, or says it showed no
// surface.
func ParseCall(output string) (Call, bool) {
	m := Marker.FindStringSubmatch(output)
	if m == nil {
		return Call{}, false
	}
	version, err := strconv.Atoi(m[2])
	if err != nil {
		return Call{}, false
	}
	call := Call{Surface: m[1], Version: version, Deleted: m[3] != "", Path: m[4]}
	session, ok := VersionPath(call.Path)
	if !ok {
		return Call{}, false
	}
	call.Session = session
	lines := strings.Split(output, "\n")
	for _, line := range lines {
		if w, ok := strings.CutPrefix(line, "warning: "); ok {
			call.Warnings = append(call.Warnings, w)
		}
	}
	for i, line := range lines {
		if !strings.HasPrefix(line, "◇ ") {
			continue
		}
		tag := " " + call.Surface + " v" + m[2]
		title := strings.TrimPrefix(line, "◇ ")
		title = strings.TrimSuffix(title, tag)
		call.Title = strings.TrimRight(title, " ")
		call.Text = strings.Join(lines[i:], "\n")
		break
	}
	return call, true
}

// VersionPath checks the path of a version, .harness/ui/<session>/<surface>/v<N>.json,
// and says its session.
func VersionPath(p string) (string, bool) {
	parts := strings.Split(p, "/")
	if len(parts) != 5 || parts[0] != ".harness" || parts[1] != "ui" || path.Clean(p) != p {
		return "", false
	}
	if !sessionPart.MatchString(parts[2]) || parts[2] == "." || parts[2] == ".." ||
		!surfaceID.MatchString(parts[3]) || !versionFile.MatchString(parts[4]) {
		return "", false
	}
	return parts[2], true
}

// maxVersion is the most of a version file read.
const maxVersion = 16 << 20

// ReadVersion reads a version of a surface from the workspace.
func ReadVersion(root *os.Root, p string) (*Object, error) {
	if _, ok := VersionPath(p); !ok {
		return nil, errors.New("not a version of a surface: " + p)
	}
	f, err := root.Open(p)
	if err != nil {
		return nil, err
	}
	defer f.Close()
	data, err := io.ReadAll(io.LimitReader(f, maxVersion+1))
	if err != nil {
		return nil, err
	}
	if len(data) > maxVersion {
		return nil, errors.New("the version is too large")
	}
	return DecodeObject(data)
}

// Components are a version's components, by ID.
func Components(version *Object) map[string]*Object {
	out := map[string]*Object{}
	for _, v := range ListOf(must(version.Get("components"))) {
		c := ObjectOf(v)
		if id := Str(c, "id"); id != "" {
			out[id] = c
		}
	}
	return out
}

// Edit is a value an input of the surface set.
type Edit struct {
	Pointer string
	Value   any
}

// Model is a version's data model: its data, then its sources and their
// sourceInfo, then the edits made in it since.
func Model(version *Object, sources, infos *Object, edits []Edit) *Object {
	doc := ObjectOf(Clone(must(version.Get("data"))))
	if doc == nil {
		doc = NewObject()
	}
	if sources == nil {
		sources = NewObject()
	}
	if infos == nil {
		infos = NewObject()
	}
	doc.Set("sources", sources)
	doc.Set("sourceInfo", infos)
	for _, e := range edits {
		if Inside(e.Pointer) {
			Set(doc, e.Pointer, Clone(e.Value))
		}
	}
	return doc
}

// Inside says whether an input may set a pointer: one outside the sources.
func Inside(pointer string) bool {
	return strings.HasPrefix(pointer, "/") && pointer != "/sources" && pointer != "/sourceInfo" &&
		!strings.HasPrefix(pointer, "/sources/") && !strings.HasPrefix(pointer, "/sourceInfo/")
}

// Snapshot is what a version kept of its sources when it was made.
func Snapshot(version *Object) (sources, infos *Object) {
	snap := ObjectOf(must(version.Get("snapshot")))
	sources = ObjectOf(Clone(must(snap.Get("sources"))))
	infos = ObjectOf(Clone(must(snap.Get("sourceInfo"))))
	return sources, infos
}

// Live reads a version's sources now.
func Live(r *Reader, root *os.Root, key string, version *Object) (sources, infos *Object) {
	sources, infos = NewObject(), NewObject()
	for _, s := range SourcesOf(version) {
		value, ok, info := r.Read(root, key, s)
		if ok {
			sources.Set(s.Name, value)
		}
		infos.Set(s.Name, info)
	}
	return sources, infos
}

// Lines is a Code's range of lines, "10-40" or "12": from and to (0: to the
// end).
func Lines(spec string) (from, to int) {
	m := regexp.MustCompile(`^(\d+)(?:-(\d+))?$`).FindStringSubmatch(strings.TrimSpace(spec))
	if m == nil {
		return 1, 0
	}
	from, _ = strconv.Atoi(m[1])
	to = from
	if m[2] != "" {
		to, _ = strconv.Atoi(m[2])
	}
	return max(from, 1), to
}

// maxCode is the most of a Code's text shown.
const maxCode = 256 << 10

// ReadCode reads the lines of a file a Code shows: its text from line
// from, and whether it was cut.
func ReadCode(root *os.Root, file, spec string) (text string, from int, cut bool, err error) {
	relative := Clean(file)
	if relative == "" {
		return "", 0, false, errors.New("a surface does not show this file")
	}
	f, err := root.Open(relative)
	if err != nil {
		return "", 0, false, err
	}
	defer f.Close()
	from, to := Lines(spec)
	var b strings.Builder
	scanner := bufio.NewScanner(io.LimitReader(f, MaxParse))
	scanner.Buffer(make([]byte, 64<<10), 4<<20)
	n := 0
	for scanner.Scan() {
		n++
		if n < from {
			continue
		}
		if to > 0 && n > to {
			break
		}
		if b.Len()+len(scanner.Bytes()) > maxCode {
			cut = true
			break
		}
		b.Write(scanner.Bytes())
		b.WriteByte('\n')
	}
	if err := scanner.Err(); err != nil {
		return "", 0, false, err
	}
	return strings.ToValidUTF8(strings.TrimSuffix(b.String(), "\n"), "\uFFFD"), from, cut, nil
}

// EventData is the data of a Button's event, with its bindings read now.
func EventData(doc any, data any) *Object {
	out := NewObject()
	o := ObjectOf(data)
	for _, key := range o.Keys() {
		raw, _ := o.Get(key)
		v, ok := Resolve(doc, raw)
		if !ok {
			v = nil
		}
		out.Set(key, Clone(v))
	}
	return out
}

// EventText is what the agent reads of an event: "[UI deps v1] explain {…}".
func EventText(surface string, version int, name string, data any) string {
	text := "[UI " + surface + " v" + strconv.Itoa(version) + "] " + name
	switch d := data.(type) {
	case nil:
	case *Object:
		if d.Len() != 0 {
			text += " " + Encode(d)
		}
	default:
		text += " " + Encode(d)
	}
	return text
}
