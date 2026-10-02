package main

import (
	"archive/zip"
	"fmt"
	"io"
	"net/http"
	"net/http/httptest"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"slices"
	"strings"
	"sync"
	"testing"
	"time"
)

func TestCompareVersions(t *testing.T) {
	for _, c := range []struct {
		a, b string
		want int
	}{
		{"v7.3.16", "v7.3.20", -1},
		{"v7.3.20", "v7.3.16", 1},
		{"v7.3.9", "v7.3.10", -1},
		{"v7.10.0", "v7.9.9", 1},
		{"v8.0.0", "v7.99.99", 1},
		{"v7.3.20", "v7.3.20", 0},
		{"v7.4.0-rc.1", "v7.4.0", -1},
		{"v7.4.0-rc.2", "v7.4.0-rc.10", -1},
		{"v7.4.0-alpha", "v7.4.0-alpha.1", -1},
		{"v7.4.0-1", "v7.4.0-alpha", -1},
		{"v0.0.0-20260926152631-39ec2650adc9", "v0.0.0-20260101000000-aaaaaaaaaaaa", 1},
		{"v7.3.20", "(devel)", 1},
		{"", "v1.0.0", -1},
	} {
		if got := compareVersions(c.a, c.b); got != c.want {
			t.Errorf("compareVersions(%q, %q) = %d, want %d", c.a, c.b, got, c.want)
		}
	}
	for v, want := range map[string]bool{
		"v7.3.20": true, "v1.0.0-rc.1": true, "v0.0.0-20260926152631-39ec2650adc9": true,
		"v2.1.3+incompatible": false, "7.3.20": false, "v7.3": false, "v07.3.20": false,
		"v7.3.20-": false, "v7.3.20-01": false, "v7.3.20-a..b": false, "(devel)": false, "": false,
	} {
		if got := validVersion(v); got != want {
			t.Errorf("validVersion(%q) = %v, want %v", v, got, want)
		}
	}
}

func TestModulePaths(t *testing.T) {
	const module = "github.com/router-for-me/CLIProxyAPI/v8"
	if got := escapeModulePath(module); got != "github.com/router-for-me/!c!l!i!proxy!a!p!i/v8" {
		t.Errorf("escapeModulePath = %q", got)
	}
	for _, c := range []struct {
		module, prefix string
		major          int
	}{
		{module, "github.com/router-for-me/CLIProxyAPI", 8},
		{"example.com/mod", "example.com/mod", 0},
		{"example.com/mod/v1", "example.com/mod/v1", 0},
		{"example.com/mod/v02", "example.com/mod/v02", 0},
		{"example.com/vendor", "example.com/vendor", 0},
	} {
		if prefix, major := splitMajor(c.module); prefix != c.prefix || major != c.major {
			t.Errorf("splitMajor(%q) = %q, %d", c.module, prefix, major)
		}
	}
	if got := moduleName(module); got != "CLIProxyAPI" {
		t.Errorf("moduleName = %q", got)
	}
	if got := releaseURL(module, "v8.0.10"); got != "https://github.com/router-for-me/CLIProxyAPI/releases/tag/v8.0.10" {
		t.Errorf("releaseURL = %q", got)
	}
	if got := compareURL(module, "v8.0.6", "v8.0.10"); got != "https://github.com/router-for-me/CLIProxyAPI/compare/v8.0.6...v8.0.10" {
		t.Errorf("compareURL = %q", got)
	}
	if got := releaseURL("example.com/gateway/v7", "v7.0.1"); got != "" {
		t.Errorf("releaseURL off GitHub = %q", got)
	}
	if !ofModule(module, "v8.0.0") || ofModule(module, "v9.0.0") || !ofModule("example.com/mod", "v1.2.0") || ofModule("example.com/mod", "v2.0.0") {
		t.Error("ofModule takes versions of another major version")
	}
}

// moduleProxy is a module proxy for modules a test makes up: each version
// of one a package of a single file, gateway.go.
type moduleProxy struct {
	*httptest.Server
	mu       sync.Mutex
	modules  map[string]map[string]string // module → version → its gateway.go
	requests map[string]int               // path → how often it was asked for
}

func newModuleProxy(t *testing.T) *moduleProxy {
	t.Helper()
	p := &moduleProxy{modules: map[string]map[string]string{}, requests: map[string]int{}}
	p.Server = httptest.NewServer(http.HandlerFunc(p.serve))
	t.Cleanup(p.Close)
	return p
}

func (p *moduleProxy) publish(module, version, source string) {
	p.mu.Lock()
	defer p.mu.Unlock()
	if p.modules[module] == nil {
		p.modules[module] = map[string]string{}
	}
	p.modules[module][version] = source
}

func (p *moduleProxy) asked(file string) int {
	p.mu.Lock()
	defer p.mu.Unlock()
	return p.requests[file]
}

func (p *moduleProxy) serve(w http.ResponseWriter, r *http.Request) {
	p.mu.Lock()
	defer p.mu.Unlock()
	p.requests[r.URL.Path]++
	module, file, _ := strings.Cut(strings.TrimPrefix(r.URL.Path, "/"), "/@")
	versions := p.modules[module]
	if versions == nil {
		http.NotFound(w, r)
		return
	}
	switch file {
	case "v/list":
		for v := range versions {
			fmt.Fprintln(w, v)
		}
		return
	case "latest":
		var list []string
		for v := range versions {
			list = append(list, v)
		}
		file = "v/" + highest(list, true) + ".info"
	}
	name, ok := strings.CutPrefix(file, "v/")
	extension := path.Ext(name)
	version := strings.TrimSuffix(name, extension)
	source, known := versions[version]
	goMod := fmt.Sprintf("module %s\n\ngo 1.21\n", module)
	switch {
	case !ok || !known:
		http.NotFound(w, r)
	case extension == ".info":
		fmt.Fprintf(w, `{"Version":%q,"Time":"2026-09-27T10:00:00Z"}`, version)
	case extension == ".mod":
		io.WriteString(w, goMod)
	case extension == ".zip":
		archive := zip.NewWriter(w)
		for _, file := range [][2]string{{"go.mod", goMod}, {"gateway.go", source}} {
			entry, err := archive.Create(module + "@" + version + "/" + file[0])
			if err != nil {
				return
			}
			io.WriteString(entry, file[1])
		}
		archive.Close()
	default:
		http.NotFound(w, r)
	}
}

// fakeUpdates has the server look for releases of example.com/gateway/v7,
// as built with running, from proxies.
func fakeUpdates(t *testing.T, proxies []string, running string, env ...string) *gatewayUpdates {
	u := newGatewayUpdates(t.Context())
	u.module, u.running, u.env = "example.com/gateway/v7", running, env
	u.proxies = func() []string { return proxies }
	return u
}

// Without a checkout to build from, the server says what is out, and what
// to run for it, but updates nothing.
func TestGatewayVersionWithoutACheckout(t *testing.T) {
	proxy := newModuleProxy(t)
	for _, v := range []string{"v7.0.0", "v7.0.1", "v7.0.2", "v7.1.0-rc.1"} {
		proxy.publish("example.com/gateway/v7", v, "package gateway\n")
	}
	proxy.publish("example.com/gateway/v8", "v8.0.0", "package gateway\n")
	// A proxy that has none of it comes first: the next one is asked.
	empty := newModuleProxy(t)
	h := newHarness(t)
	h.server.updates = fakeUpdates(t, []string{empty.URL, proxy.URL}, "v7.0.0")

	res, body := h.do("GET", "/api/gateway/update", "")
	if res.StatusCode != http.StatusOK {
		t.Fatalf("GET: %d %v", res.StatusCode, body)
	}
	latest, _ := body["latest"].(map[string]any)
	major, _ := body["major"].(map[string]any)
	if body["name"] != "gateway" || body["running"] != "v7.0.0" || body["available"] != true || body["behind"] != 2.0 ||
		latest["version"] != "v7.0.2" || latest["time"] == nil || major["version"] != "v8.0.0" || body["error"] != nil {
		t.Errorf("GET = %v", body)
	}
	if body["can_update"] != false || !strings.Contains(fmt.Sprint(body["reason"]), "checkout") ||
		body["command"] != "go get example.com/gateway/v7@v7.0.2" || body["required"] != nil {
		t.Errorf("without a checkout: %v", body)
	}
	// What the proxies said holds for a while; ?refresh=1 asks again.
	h.do("GET", "/api/gateway/update", "")
	if n := proxy.asked("/example.com/gateway/v7/@v/list"); n != 1 {
		t.Errorf("the proxy was asked %d times, want 1", n)
	}
	h.do("GET", "/api/gateway/update?refresh=1", "")
	if n := proxy.asked("/example.com/gateway/v7/@v/list"); n != 2 {
		t.Errorf("refreshed, the proxy was asked %d times, want 2", n)
	}
	if res, body := h.do("POST", "/api/gateway/update", `{"version":"v7.0.2"}`); res.StatusCode != http.StatusConflict || !strings.Contains(fmt.Sprint(body["error"]), "checkout") {
		t.Errorf("POST without a checkout: %d %v", res.StatusCode, body)
	}
	if res, _ := h.do("POST", "/api/gateway/update", `{"version":"latest"}`); res.StatusCode != http.StatusBadRequest {
		t.Errorf("POST latest: %d, want 400", res.StatusCode)
	}

	h.server.updates = fakeUpdates(t, []string{proxy.URL}, "v7.0.2")
	if _, body := h.do("GET", "/api/gateway/update", ""); body["available"] != false || body["behind"] != nil || body["command"] != nil {
		t.Errorf("up to date: %v", body)
	}
	h.server.updates = fakeUpdates(t, nil, "v7.0.0")
	if _, body := h.do("GET", "/api/gateway/update", ""); body["available"] != false || !strings.Contains(fmt.Sprint(body["error"]), "GOPROXY") {
		t.Errorf("no proxy to ask: %v", body)
	}
}

// A server that builds itself from its checkout moves go.mod and go.sum to
// the latest release once its programs build with it — and not before.
func TestGatewayUpdate(t *testing.T) {
	if testing.Short() {
		t.Skip("runs go get and go build")
	}
	gobin, err := exec.LookPath("go")
	if err != nil {
		t.Skip("no go on the PATH")
	}
	const module = "example.com/gateway/v7"
	hello := "package gateway\n\nfunc Hello() string { return \"hello\" }\n"
	proxy := newModuleProxy(t)
	proxy.publish(module, "v7.0.0", hello)
	proxy.publish(module, "v7.0.1", hello)
	env := []string{
		"GOPROXY=" + proxy.URL, "GOSUMDB=off", "GONOSUMDB=", "GONOPROXY=", "GOPRIVATE=", "GOTOOLCHAIN=local",
		"GOFLAGS=-modcacherw", "GOMODCACHE=" + t.TempDir(),
	}
	root := t.TempDir()
	writeFiles(t, root, map[string]string{
		"go.mod":                       "module example.com/checkout\n\ngo 1.21\n",
		"cmd/kou-conveyor-web/main.go": "package main\n\nimport \"example.com/gateway/v7\"\n\nfunc main() { println(gateway.Hello()) }\n",
	})
	get := exec.Command(gobin, "get", module+"@v7.0.0")
	get.Dir, get.Env = root, append(os.Environ(), env...)
	if output, err := get.CombinedOutput(); err != nil {
		t.Fatalf("go get: %v\n%s", err, output)
	}
	installed := t.TempDir()
	h := newHarness(t)
	h.server.rebuild = &rebuilder{root: root, dir: installed, exe: filepath.Join(installed, "kou-conveyor-web")}
	h.server.updates = fakeUpdates(t, []string{proxy.URL}, "v7.0.0", env...)

	_, body := h.do("GET", "/api/gateway/update", "")
	if body["required"] != "v7.0.0" || body["available"] != true || body["behind"] != 1.0 || body["can_update"] != true || body["checkout"] != display(root) {
		t.Fatalf("GET = %v", body)
	}
	// Only the release the page was offered is taken up.
	if res, body := h.do("POST", "/api/gateway/update", `{"version":"v7.0.0"}`); res.StatusCode != http.StatusConflict {
		t.Errorf("POST v7.0.0: %d %v", res.StatusCode, body)
	}
	res, body := h.do("POST", "/api/gateway/update", `{"version":"v7.0.1"}`)
	if job, _ := body["update"].(map[string]any); res.StatusCode != http.StatusAccepted || job["state"] != "updating" || body["can_update"] != false {
		t.Fatalf("POST v7.0.1: %d %v", res.StatusCode, body)
	}
	job := waitForUpdate(h)
	if job["state"] != "updated" || job["version"] != "v7.0.1" || !strings.Contains(fmt.Sprint(job["output"]), "upgraded example.com/gateway/v7 v7.0.0 => v7.0.1") {
		t.Fatalf("update = %v", job)
	}
	mod, _ := os.ReadFile(filepath.Join(root, "go.mod"))
	sum, _ := os.ReadFile(filepath.Join(root, "go.sum"))
	if !strings.Contains(string(mod), module+" v7.0.1") || !strings.Contains(string(sum), module+" v7.0.1 h1:") {
		t.Fatalf("after the update:\n%s\n%s", mod, sum)
	}
	// The page follows the server's build of itself with it from there.
	if _, body := h.do("GET", "/api/gateway/update", ""); body["required"] != "v7.0.1" || body["available"] != false || body["build"] == nil {
		t.Errorf("GET after the update = %v", body)
	}

	// A release the programs do not build with leaves go.mod and go.sum as
	// they were.
	proxy.publish(module, "v7.1.0", "package gateway\n\nfunc Goodbye() string { return \"goodbye\" }\n")
	if _, body := h.do("GET", "/api/gateway/update?refresh=1", ""); body["available"] != true {
		t.Fatalf("GET with v7.1.0 out = %v", body)
	}
	if res, body := h.do("POST", "/api/gateway/update", `{"version":"v7.1.0"}`); res.StatusCode != http.StatusAccepted {
		t.Fatalf("POST v7.1.0: %d %v", res.StatusCode, body)
	}
	job = waitForUpdate(h)
	if job["state"] != "failed" || !strings.Contains(fmt.Sprint(job["message"]), "stay as they were") || !strings.Contains(fmt.Sprint(job["output"]), "Hello") {
		t.Errorf("update to v7.1.0 = %v", job)
	}
	if now, _ := os.ReadFile(filepath.Join(root, "go.mod")); string(now) != string(mod) {
		t.Errorf("go.mod changed:\n%s", now)
	}
	if now, _ := os.ReadFile(filepath.Join(root, "go.sum")); string(now) != string(sum) {
		t.Errorf("go.sum changed:\n%s", now)
	}
	entries, _ := os.ReadDir(root)
	var names []string
	for _, entry := range entries {
		names = append(names, entry.Name())
	}
	if !slices.Equal(names, []string{"cmd", "go.mod", "go.sum"}) {
		t.Errorf("the checkout holds %v", names)
	}
}

// waitForUpdate waits for the update under way to end, and returns how it
// ended.
func waitForUpdate(h *harness) map[string]any {
	h.Helper()
	for deadline := time.Now().Add(3 * time.Minute); time.Now().Before(deadline); time.Sleep(100 * time.Millisecond) {
		_, body := h.do("GET", "/api/gateway/update", "")
		if job, _ := body["update"].(map[string]any); job != nil && job["state"] != "updating" {
			return job
		}
	}
	h.Fatal("the update did not end")
	return nil
}
