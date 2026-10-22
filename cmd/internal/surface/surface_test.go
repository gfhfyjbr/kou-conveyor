package surface

import (
	"math"
	"os"
	"path/filepath"
	"strings"
	"testing"
)

func decode(t *testing.T, text string) any {
	t.Helper()
	v, err := Decode([]byte(text))
	if err != nil {
		t.Fatalf("decode %s: %v", text, err)
	}
	return v
}

func TestNum(t *testing.T) {
	for _, tc := range []struct {
		v      float64
		digits int
		want   string
	}{
		{1204, -1, "1 204"},
		{41.6, -1, "41.6"},
		{8.5, -1, "8.5"},
		{0.1234, -1, "0.12"},
		{12.34, -1, "12.3"},
		{12, -1, "12"},
		{123.456, -1, "123"},
		{-1234567.891, -1, "-1 234 568"},
		{0.125, 2, "0.13"}, // a tie goes up, as toFixed
		{1.005, 2, "1.00"}, // 1.005 is a little less
		{2.5, 0, "3"},
		{0.5, -1, "0.5"},
		{3, 2, "3.00"},
		{1e6, -1, "1 000 000"},
		{math.NaN(), -1, "nan"},
		{math.Inf(1), -1, "inf"},
		{math.Inf(-1), 1, "-inf"},
	} {
		if got := Num(tc.v, tc.digits); got != tc.want {
			t.Errorf("Num(%v, %d) = %q, want %q", tc.v, tc.digits, got, tc.want)
		}
	}
}

func TestDelta(t *testing.T) {
	for _, tc := range []struct {
		v, p any
		want string
		dir  int
		none bool
	}{
		{41.6, 40.0, "▲4%", 1, false},
		{99.5, 100.0, "▼0.5%", -1, false},
		{100.01, 100.0, "=0%", 0, false},
		{5.0, 0.0, "", 0, true},
		{"12", "10", "▲20%", 1, false},
		{"x", 10.0, "", 0, true},
		{-5.0, -10.0, "▲50%", 1, false},
	} {
		got, ok := Delta(tc.v, true, tc.p, true)
		if ok == tc.none || got.Text != tc.want || got.Dir != tc.dir {
			t.Errorf("Delta(%v, %v) = %+v %v, want %q %d", tc.v, tc.p, got, ok, tc.want, tc.dir)
		}
	}
	if _, ok := Delta(1.0, false, 2.0, true); ok {
		t.Error("a missing value has no change")
	}
}

func TestPointer(t *testing.T) {
	doc := decode(t, `{"a":[1,2,3],"b":{"c/d":4,"e~f":5},"n":null}`)
	for _, tc := range []struct {
		ptr  string
		want string
		ok   bool
	}{
		{"/a/-1", "3", true},
		{"/a/-2", "2", true},
		{"/a/0", "1", true},
		{"/a/3", "", false},
		{"/a/-4", "", false},
		{"/a/x", "", false},
		{"/b/c~1d", "4", true},
		{"/b/e~0f", "5", true},
		{"/n", "null", true},
		{"/z", "", false},
		{"/a/0/x", "", false},
		{"a", "", false},
	} {
		v, ok := Get(doc, tc.ptr)
		if ok != tc.ok || (ok && Encode(v) != tc.want) {
			t.Errorf("Get(%q) = %s %v, want %s %v", tc.ptr, Encode(v), ok, tc.want, tc.ok)
		}
	}
	if v, ok := Get(doc, ""); !ok || v != doc {
		t.Error(`Get("") is the document`)
	}

	o := ObjectOf(doc)
	Set(o, "/x/y", "new")
	Set(o, "/a/1", nil)
	Set(o, "/a/-", 9.0)
	Set(o, "/b/c~1d", nil)
	Set(o, "/a/7", 1.0) // out of the list: nothing
	if got, want := Encode(o), `{"a":[1,3,9],"b":{"e~f":5},"n":null,"x":{"y":"new"}}`; got != want {
		t.Errorf("after Set: %s, want %s", got, want)
	}
}

func TestDecodeEncode(t *testing.T) {
	v := decode(t, `{"b":1,"a":[true,false,null,"é\n\u0001",0.1,1e21,1e-7,-0],"b":2}`)
	if got, want := Encode(v), `{"b":2,"a":[true,false,null,"é\n\u0001",0.1,1e+21,1e-7,0]}`; got != want {
		t.Errorf("Encode = %s, want %s", got, want)
	}
	if v := decode(t, `[1e400]`); !math.IsInf(ListOf(v)[0].(float64), 1) {
		t.Errorf("1e400 = %v, want +Inf", v)
	}
	for _, bad := range []string{`{`, `[1,]`, `{"a":1} x`, `nope`, ``} {
		if _, err := Decode([]byte(bad)); err == nil {
			t.Errorf("Decode(%q) took it", bad)
		}
	}
	if _, err := Decode([]byte(strings.Repeat("[", 600) + strings.Repeat("]", 600))); err == nil {
		t.Error("Decode took 600 lists deep")
	}
}

func TestBinding(t *testing.T) {
	if p, ok := Binding(decode(t, `{"path":"/x"}`)); !ok || p != "/x" {
		t.Error("a binding")
	}
	for _, not := range []string{`{"path":"/x","y":1}`, `{"path":1}`, `"/x"`, `["/x"]`} {
		if _, ok := Binding(decode(t, not)); ok {
			t.Errorf("%s is no binding", not)
		}
	}
}

func TestMetricAndFill(t *testing.T) {
	doc := decode(t, `{"sources":{"runs":{"rows":[{"p99":40},{"p99":41.6}],"last":{"p99":41.6},"count":2}},"name":"bench","n":3}`)
	comps := map[string]*Object{
		"p99":  ObjectOf(decode(t, `{"id":"p99","component":"Metric","label":"p99","unit":"ms","better":"lower","value":{"path":"/sources/runs/last/p99"}}`)),
		"miss": ObjectOf(decode(t, `{"id":"miss","component":"Metric","unit":"ms","value":{"path":"/sources/none/last/x"}}`)),
		"own":  ObjectOf(decode(t, `{"id":"own","component":"Metric","value":12.345,"digits":1,"previous":10,"better":"higher"}`)),
	}
	m := Metric(doc, comps["p99"])
	if m.Text != "41.6" || m.Unit != "ms" || m.Change == nil || m.Change.Text != "▲4%" || m.Tone != "bad" || m.Missing {
		t.Errorf("Metric p99 = %+v", m)
	}
	m = Metric(doc, comps["miss"])
	if m.Text != "—" || m.Unit != "" || m.Change != nil || !m.Missing || m.Tone != "flat" {
		t.Errorf("Metric miss = %+v", m)
	}
	m = Metric(doc, comps["own"])
	if m.Text != "12.3" || m.Change.Text != "▲23%" || m.Tone != "good" {
		t.Errorf("Metric own = %+v", m)
	}
	if got, want := Fill(doc, comps, "p99 {#p99} of {/name} ×{/n} {/none} {#nope} {x}"), "p99 41.6 ms ▲4% of bench ×3 — ? {x}"; got != want {
		t.Errorf("Fill = %q, want %q", got, want)
	}
	if got := TextOf(doc, comps, decode(t, `{"path":"/none"}`)); got != "—" {
		t.Errorf("TextOf missing = %q", got)
	}
	if got := TextOf(doc, comps, decode(t, `{"path":"/sources/runs/last"}`)); got != `{"p99":41.6}` {
		t.Errorf("TextOf object = %q", got)
	}
	if got := FillItem(decode(t, `{"name":"a","ok":true}`), "{/name}: {/ok} {#p99}"); got != "a: true {#p99}" {
		t.Errorf("FillItem = %q", got)
	}
}

func TestParseSource(t *testing.T) {
	v, err := ParseSource("csv", "a, b ,\n1,x,7\n\n  ,  \n2.5,\"y,\"\"z\"\"\"\n-3,\"multi\nline\"\n")
	if err != nil {
		t.Fatal(err)
	}
	if got, want := Encode(v), `{"columns":["a","b",""],"rows":[{"a":1,"b":"x","col3":7},{"a":2.5,"b":"y,\"z\""},{"a":-3,"b":"multi\nline"}],"last":{"a":-3,"b":"multi\nline"},"count":3}`; got != want {
		t.Errorf("csv = %s\nwant %s", got, want)
	}
	v, _ = ParseSource("tsv", "k\tv\r\nx\t1e3\r\n")
	if got, want := Encode(v), `{"columns":["k","v"],"rows":[{"k":"x","v":1000}],"last":{"k":"x","v":1000},"count":1}`; got != want {
		t.Errorf("tsv = %s", got)
	}
	v, _ = ParseSource("jsonl", "{\"a\":1}\n\nnot json\r\n{\"a\":2}\u2028{\"a\":3}\n{\"a\":")
	if got, want := Encode(v), `{"rows":[{"a":1},{"a":2},{"a":3}],"last":{"a":3},"count":3}`; got != want {
		t.Errorf("jsonl = %s", got)
	}
	v, _ = ParseSource("jsonl", "")
	if got, want := Encode(v), `{"rows":[],"last":null,"count":0}`; got != want {
		t.Errorf("empty jsonl = %s", got)
	}
	v, _ = ParseSource("csv", "")
	if got, want := Encode(v), `{"columns":[],"rows":[],"last":null,"count":0}`; got != want {
		t.Errorf("empty csv = %s", got)
	}
	if _, err := ParseSource("json", "{nope"); err == nil {
		t.Error("json took {nope")
	}
	v, _ = ParseSource("markdown", strings.Repeat("é", MaxText+5))
	if n := len([]rune(v.(string))); n != MaxText {
		t.Errorf("text kept %d characters", n)
	}

	var many strings.Builder
	for i := range MaxRows + 5 {
		many.WriteString(`{"i":` + Num(float64(i), 0) + "}\n")
	}
	v, _ = ParseSource("jsonl", strings.ReplaceAll(many.String(), " ", ""))
	o := ObjectOf(v)
	if c, _ := Int(o, "count"); c != MaxRows+5 || len(ListOf(must(o.Get("rows")))) != MaxRows {
		t.Errorf("jsonl of %d rows: count %v, rows %d", MaxRows+5, c, len(ListOf(must(o.Get("rows")))))
	}
	if first, _ := Get(o, "/rows/0/i"); first != 5.0 {
		t.Errorf("jsonl keeps the last rows: first %v", first)
	}
}

func TestClean(t *testing.T) {
	for in, want := range map[string]string{
		"a/b.json":       "a/b.json",
		" $cmd/main.go ": "cmd/main.go",
		"a/../b":         "b",
		"./x":            "x",
		"../x":           "",
		"a/../../x":      "",
		"/etc/passwd":    "",
		".git/config":    "",
		".env":           "",
		"app/.env.local": "",
		"x\\y":           "",
		"":               "",
		".":              "",
		".github/ci.yml": ".github/ci.yml",
		".envrc":         ".envrc",
	} {
		if got := Clean(in); got != want {
			t.Errorf("Clean(%q) = %q, want %q", in, got, want)
		}
	}
}

const call = "UI deps v2 (.harness/ui/67264553-1f58-4d7c-89ad-befafd2ccb16/deps/v2.json)\n" +
	"warning: map: a pointer of inputs is not set\n" +
	"◇ go.mod — dependency map                         deps v2\n" +
	"  Filter: [ pion ]"

func TestParseCall(t *testing.T) {
	c, ok := ParseCall(call)
	if !ok {
		t.Fatal("no call")
	}
	if c.Surface != "deps" || c.Version != 2 || c.Deleted || c.Session != "67264553-1f58-4d7c-89ad-befafd2ccb16" ||
		c.Path != ".harness/ui/67264553-1f58-4d7c-89ad-befafd2ccb16/deps/v2.json" || c.Title != "go.mod — dependency map" ||
		len(c.Warnings) != 1 || !strings.HasPrefix(c.Text, "◇ go.mod") || !strings.HasSuffix(c.Text, "[ pion ]") {
		t.Errorf("ParseCall = %+v", c)
	}
	c, ok = ParseCall("UI deps v3 deleted (.harness/ui/s1/deps/v3.json)")
	if !ok || !c.Deleted || c.Version != 3 || c.Title != "" {
		t.Errorf("deleted = %+v %v", c, ok)
	}
	for _, not := range []string{
		"UI deps: nothing shown\n- components: none",
		"UI deps v1 (.harness/ui/s/other/v1.json/../../x.json)",
		"UI deps v1 (.harness/ui/s/deps/latest.json)",
		"error\nUI deps v1 (.harness/ui/s/deps/v1.json)",
	} {
		if c, ok := ParseCall(not); ok {
			t.Errorf("ParseCall(%q) = %+v", not, c)
		}
	}
	if !IsTool("UI") || !IsTool("onion_UI") || !IsTool("mcp__assault_kidney__onion_UI") || IsTool("GUI") || IsTool("UIX") || IsTool("Bash") {
		t.Error("IsTool")
	}
}

func TestVersionPath(t *testing.T) {
	for p, want := range map[string]bool{
		".harness/ui/s/deps/v1.json":       true,
		".harness/ui/a.b_c-d/x_1/v12.json": true,
		".harness/ui/s/deps/latest.json":   false,
		".harness/ui/s/deps/v0.json":       false,
		".harness/ui/../deps/v1.json":      false,
		".harness/ui/s/Deps/v1.json":       false,
		".harness/ui/s/deps/x/v1.json":     false,
		"x/ui/s/deps/v1.json":              false,
	} {
		if _, ok := VersionPath(p); ok != want {
			t.Errorf("VersionPath(%q) = %v", p, ok)
		}
	}
}

func write(t *testing.T, dir, rel, text string) {
	t.Helper()
	full := filepath.Join(dir, filepath.FromSlash(rel))
	if err := os.MkdirAll(filepath.Dir(full), 0o755); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(full, []byte(text), 0o644); err != nil {
		t.Fatal(err)
	}
}

func TestVersionAndSources(t *testing.T) {
	dir := t.TempDir()
	write(t, dir, ".harness/ui/s1/bench/v1.json", `{"kou":"genui/v0","surface":"bench","version":1,
		"sources":{"runs":{"file":"bench/runs.jsonl"},"notes":{"file":"NOTES.md","format":"text"},"gone":{"file":"nope.csv"},
		"dir":{"file":"bench"},"out":{"file":"../x.json"},"bad":{"file":"bad.json"}},
		"components":[{"id":"root","component":"Column","children":["m"]},{"id":"m","component":"Metric","value":{"path":"/sources/runs/last/p"}}],
		"data":{"filter":"a","nested":{"x":1}},
		"snapshot":{"sources":{"runs":{"rows":[{"p":1}],"last":{"p":1},"count":1}},"sourceInfo":{"runs":{"path":"bench/runs.jsonl","size":8}}}}`)
	write(t, dir, "bench/runs.jsonl", "{\"p\":1}\n{\"p\":2}\n")
	write(t, dir, "NOTES.md", "# notes")
	write(t, dir, "bad.json", "{")
	root, err := os.OpenRoot(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()

	version, err := ReadVersion(root, ".harness/ui/s1/bench/v1.json")
	if err != nil {
		t.Fatal(err)
	}
	if _, err := ReadVersion(root, ".harness/ui/s1/bench/latest.json"); err == nil {
		t.Error("ReadVersion read latest.json")
	}
	if comps := Components(version); len(comps) != 2 || Str(comps["m"], "component") != "Metric" {
		t.Errorf("Components = %v", comps)
	}
	srcs := SourcesOf(version)
	if len(srcs) != 6 || srcs[0] != (Source{"runs", "bench/runs.jsonl", "jsonl"}) || srcs[1].Format != "text" || srcs[2].Format != "csv" {
		t.Errorf("SourcesOf = %+v", srcs)
	}

	r := NewReader()
	sources, infos := Live(r, root, "s1/bench", version)
	if got, want := Encode(sources), `{"runs":{"rows":[{"p":1},{"p":2}],"last":{"p":2},"count":2},"notes":"# notes"}`; got != want {
		t.Errorf("Live sources = %s", got)
	}
	errs := map[string]string{"gone": "no such file", "dir": "a folder, not a file", "out": "the file leads outside the workspace"}
	for name, want := range errs {
		if got := Str(ObjectOf(must(infos.Get(name))), "error"); got != want {
			t.Errorf("%s: error %q, want %q", name, got, want)
		}
	}
	if got := Str(ObjectOf(must(infos.Get("bad"))), "error"); !strings.HasPrefix(got, "not json: ") {
		t.Errorf("bad: error %q", got)
	}
	runs := ObjectOf(must(infos.Get("runs")))
	if Str(runs, "mtime") == "" || !strings.HasSuffix(Str(runs, "mtime"), "Z") {
		t.Errorf("runs info = %s", Encode(runs))
	}
	if size, _ := Int(runs, "size"); size != 16 {
		t.Errorf("runs size %d", size)
	}

	// Read again: the same values, from what it read before.
	again, _ := Live(r, root, "s1/bench", version)
	if v1, _ := again.Get("runs"); v1 != must(sources.Get("runs")) {
		t.Error("the reader read an unchanged file again")
	}
	write(t, dir, "bench/runs.jsonl", "{\"p\":1}\n{\"p\":2}\n{\"p\":3}\n")
	again, _ = Live(r, root, "s1/bench", version)
	if got, _ := Get(again, "/runs/last/p"); got != 3.0 {
		t.Errorf("after the file changed: last %v", got)
	}

	doc := Model(version, again, infos, []Edit{{"/filter", "b"}, {"/sources/runs", "no"}, {"/nested/y", true}, {"/nested/x", nil}})
	if got, want := Encode(must(doc.Get("filter"))), `"b"`; got != want {
		t.Errorf("filter = %s", got)
	}
	if got := Encode(must(doc.Get("nested"))); got != `{"y":true}` {
		t.Errorf("nested = %s", got)
	}
	if _, ok := Get(doc, "/sources/runs/rows"); !ok {
		t.Error("an edit set a source")
	}
	if got := Encode(must(version.Get("data"))); got != `{"filter":"a","nested":{"x":1}}` {
		t.Errorf("Model changed the version's data: %s", got)
	}
	snap, snapInfo := Snapshot(version)
	if got, _ := Get(snap, "/runs/last/p"); got != 1.0 || snapInfo.Len() != 1 {
		t.Errorf("Snapshot = %s %s", Encode(snap), Encode(snapInfo))
	}
}

func TestBigSources(t *testing.T) {
	dir := t.TempDir()
	var b strings.Builder
	for b.Len() <= MaxParse+100 {
		b.WriteString(`{"n":12345678901234567890}` + "\n")
	}
	b.WriteString(`{"n":1}` + "\n")
	write(t, dir, "big.jsonl", b.String())
	write(t, dir, "big.json", `"`+strings.Repeat("x", MaxParse)+`"`)
	root, err := os.OpenRoot(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	v, ok, info := ReadSource(root, Source{"big", "big.jsonl", "jsonl"})
	if !ok || !Bool(info, "cut") {
		t.Fatalf("big jsonl: %v %s", ok, Encode(info))
	}
	if last, _ := Get(v, "/last/n"); last != 1.0 {
		t.Errorf("big jsonl last = %v", last)
	}
	if _, ok, info := ReadSource(root, Source{"big", "big.json", "json"}); ok || Str(info, "error") != "bigger than 5 MB" {
		t.Errorf("big json: %v %s", ok, Encode(info))
	}
}

func TestCode(t *testing.T) {
	dir := t.TempDir()
	write(t, dir, "a.go", "one\ntwo\nthree\nfour\n")
	root, err := os.OpenRoot(dir)
	if err != nil {
		t.Fatal(err)
	}
	defer root.Close()
	for spec, want := range map[string]string{"": "one\ntwo\nthree\nfour", "2-3": "two\nthree", "3": "three", "x": "one\ntwo\nthree\nfour", "9": ""} {
		text, _, _, err := ReadCode(root, "a.go", spec)
		if err != nil || text != want {
			t.Errorf("ReadCode(%q) = %q %v, want %q", spec, text, err, want)
		}
	}
	if _, _, _, err := ReadCode(root, ".env", ""); err == nil {
		t.Error("ReadCode read .env")
	}
}

func TestEvent(t *testing.T) {
	doc := decode(t, `{"comment":"after 18:00","mode":"batch"}`)
	data := EventData(doc, decode(t, `{"comment":{"path":"/comment"},"mode":{"path":"/mode"},"none":{"path":"/x"},"lit":5}`))
	if got, want := EventText("migration", 1, "apply", data), `[UI migration v1] apply {"comment":"after 18:00","mode":"batch","none":null,"lit":5}`; got != want {
		t.Errorf("EventText = %s", got)
	}
	if got := EventText("m", 2, "cancel", EventData(doc, nil)); got != "[UI m v2] cancel" {
		t.Errorf("EventText empty = %s", got)
	}
}
