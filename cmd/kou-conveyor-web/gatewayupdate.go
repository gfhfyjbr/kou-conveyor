package main

import (
	"bytes"
	"cmp"
	"context"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io"
	"io/fs"
	"net/http"
	"os"
	"os/exec"
	"path"
	"path/filepath"
	"strings"
	"sync"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/accounts"
)

// The accounts gateway is CLIProxyAPI, compiled into the server as a Go
// module (accounts.go). Pages ask, once they open, whether a newer release
// of it is out than the one go.mod requires: the server asks the module
// proxies GOPROXY names, as go does, at most once an hour. A server that
// builds itself from its checkout (rebuild.go) can take the latest release
// up: go get moves copies of go.mod and go.sum to it, the programs are built
// with the copies, and only once they build do the copies take the place of
// go.mod and go.sum — a change the server follows as it follows any change
// of its Go code, building itself anew and restarting once no agent is at
// work. A newer major version is another module, whose API changed, and is
// only told of.

const (
	updateFresh     = time.Hour        // how long what a check found holds
	updateRetry     = 10 * time.Minute // how long a check that failed holds
	updateCheckTime = 20 * time.Second // how long a check may take
	updateMajorTime = 6 * time.Second  // of which, the look for a newer major version
	updateFetchTime = 5 * time.Minute  // how long go get may take
	updateBuildTime = 10 * time.Minute // how long the programs may take to build
)

// errNoModule says that no module proxy has a module.
var errNoModule = errors.New("no module proxy has the module")

// moduleRelease is a release of a module.
type moduleRelease struct {
	Path    string    `json:"path"`
	Version string    `json:"version"`
	Time    time.Time `json:"time,omitzero"`
	// URL is the release's page, for a module on GitHub.
	URL string `json:"url,omitzero"`
}

// updateJob is what the update of the gateway does, or did last.
type updateJob struct {
	State   string `json:"state"` // idle, updating, updated, failed
	Version string `json:"version,omitzero"`
	Message string `json:"message,omitzero"`
	// Output is go's, when the update failed, and what go get moved when
	// it did not.
	Output string    `json:"output,omitzero"`
	At     time.Time `json:"at,omitzero"`
}

// updateStatus is what pages learn of the gateway's version.
type updateStatus struct {
	Module string `json:"module"`
	Name   string `json:"name"`
	// Running is the version the server was built with; Required, the one
	// the checkout's go.mod requires.
	Running  string `json:"running,omitzero"`
	Required string `json:"required,omitzero"`
	// Latest is the module's latest release; Available, that it is newer
	// than the version required — or, without a checkout, the one running —
	// by Behind releases, whose changes Changes shows.
	Latest    *moduleRelease `json:"latest,omitzero"`
	Available bool           `json:"available"`
	Behind    int            `json:"behind,omitzero"`
	Changes   string         `json:"changes,omitzero"`
	// Major is the latest release of a newer major version: another module.
	Major *moduleRelease `json:"major,omitzero"`
	// CanUpdate says that the server can take the latest release up itself;
	// Reason, why it cannot, and Command what to run in a checkout instead.
	CanUpdate bool   `json:"can_update"`
	Reason    string `json:"reason,omitzero"`
	Checkout  string `json:"checkout,omitzero"`
	Command   string `json:"command,omitzero"`
	// CheckedAt is when the module proxies were asked; Error, why they did
	// not say.
	CheckedAt time.Time `json:"checked_at,omitzero"`
	Error     string    `json:"error,omitzero"`
	Update    updateJob `json:"update"`
	// Build is what the server's build of itself did last (rebuild.go),
	// once an update moved go.mod.
	Build *buildStatus `json:"build,omitzero"`
}

// gatewayUpdates looks for newer releases of the gateway, and takes one up.
type gatewayUpdates struct {
	module  string          // the gateway's Go module
	running string          // the version of it the server was built with
	proxies func() []string // the module proxies to ask
	env     []string        // for go commands, beside the server's environment
	client  *http.Client
	life    context.Context // the server's: checks and updates end with it

	mu       sync.Mutex
	found    moduleCheck   // what the latest check found
	checking chan struct{} // closed once the check under way ends
	job      updateJob
}

// moduleCheck is what a check of the module proxies found.
type moduleCheck struct {
	at       time.Time
	err      error
	latest   *moduleRelease
	versions []string // those the module proxy lists
	major    *moduleRelease
}

func newGatewayUpdates(life context.Context) *gatewayUpdates {
	return &gatewayUpdates{
		module: accounts.Module, running: accounts.Version(), proxies: goProxies,
		client: &http.Client{Timeout: 15 * time.Second}, life: life,
		job: updateJob{State: "idle"},
	}
}

// ---------------------------------------------------------------- checks

// check returns what the module proxies say of the gateway's module: what
// a check found within the hour — ten minutes, when it failed — unless
// refresh asks again. A check under way is waited for rather than doubled,
// and goes on when ctx ends, for the next request to find.
func (u *gatewayUpdates) check(ctx context.Context, refresh bool) moduleCheck {
	u.mu.Lock()
	hold := updateFresh
	if u.found.err != nil {
		hold = updateRetry
	}
	if !refresh && !u.found.at.IsZero() && time.Since(u.found.at) < hold {
		defer u.mu.Unlock()
		return u.found
	}
	done := u.checking
	if done == nil {
		done = make(chan struct{})
		u.checking = done
		go func() {
			found := u.lookup(u.life)
			u.mu.Lock()
			u.found, u.checking = found, nil
			u.mu.Unlock()
			close(done)
		}()
	}
	u.mu.Unlock()
	select {
	case <-done:
	case <-ctx.Done():
	}
	u.mu.Lock()
	defer u.mu.Unlock()
	return u.found
}

// lookup asks the module proxies for the module's latest release and its
// versions, and for the latest release of a newer major version, if one is
// out.
func (u *gatewayUpdates) lookup(ctx context.Context) moduleCheck {
	ctx, cancel := context.WithTimeout(ctx, updateCheckTime)
	defer cancel()
	proxies := u.proxies()
	if len(proxies) == 0 {
		return moduleCheck{at: time.Now(), err: errors.New("GOPROXY names no module proxy to ask")}
	}
	var found moduleCheck
	var wg sync.WaitGroup
	wg.Go(func() { found.latest, found.versions, found.err = u.latest(ctx, proxies, u.module) })
	wg.Go(func() {
		ctx, cancel := context.WithTimeout(ctx, updateMajorTime)
		defer cancel()
		found.major = u.newerMajor(ctx, proxies)
	})
	wg.Wait()
	found.at = time.Now()
	return found
}

// latest finds a module's latest release as go get does: the highest
// release the module proxy lists — or, when it lists none, its highest
// pre-release, or else what its @latest says — with the versions it lists.
func (u *gatewayUpdates) latest(ctx context.Context, proxies []string, module string) (*moduleRelease, []string, error) {
	list, err := u.ask(ctx, proxies, module, "@v/list")
	if err != nil {
		return nil, nil, err
	}
	var versions []string
	for line := range strings.Lines(string(list)) {
		if v := strings.TrimSpace(line); ofModule(module, v) {
			versions = append(versions, v)
		}
	}
	best := highest(versions, true)
	if best == "" {
		best = highest(versions, false)
	}
	file := "@latest"
	if best != "" {
		file = "@v/" + escapeModulePath(best) + ".info"
	}
	release := &moduleRelease{Path: module, Version: best}
	info, err := u.ask(ctx, proxies, module, file)
	if err == nil {
		var answer struct {
			Version string
			Time    time.Time
		}
		if json.Unmarshal(info, &answer) == nil && ofModule(module, answer.Version) && (best == "" || answer.Version == best) {
			release.Version, release.Time = answer.Version, answer.Time
		}
	}
	if release.Version == "" {
		if err == nil {
			err = fmt.Errorf("the module proxy names no version of %s", module)
		}
		return nil, versions, err
	}
	release.URL = releaseURL(module, release.Version)
	return release, versions, nil
}

// newerMajor finds the latest release of the newest major version of the
// gateway that is out: another module, path/v8 for path/v7. It looks up to
// five major versions ahead, for as long as ctx lasts.
func (u *gatewayUpdates) newerMajor(ctx context.Context, proxies []string) *moduleRelease {
	prefix, major := splitMajor(u.module)
	major = max(major, 1)
	var newest *moduleRelease
	for next := major + 1; next <= major+5; next++ {
		release, _, err := u.latest(ctx, proxies, fmt.Sprintf("%s/v%d", prefix, next))
		if err != nil || !isRelease(release.Version) {
			break
		}
		newest = release
	}
	return newest
}

// ask fetches a file of a module — @v/list, @latest, @v/<version>.info —
// from the first module proxy that has it.
func (u *gatewayUpdates) ask(ctx context.Context, proxies []string, module, file string) ([]byte, error) {
	err := errNoModule
	for _, proxy := range proxies {
		body, status, fetchErr := u.fetch(ctx, strings.TrimRight(proxy, "/")+"/"+escapeModulePath(module)+"/"+file)
		switch {
		case fetchErr != nil:
			err = fetchErr
		case status == http.StatusOK:
			return body, nil
		case status == http.StatusNotFound || status == http.StatusGone:
			// This proxy has no such module; the next may.
		default:
			err = fmt.Errorf("%s answered %d %s", proxy, status, http.StatusText(status))
		}
		if ctx.Err() != nil {
			break
		}
	}
	return nil, err
}

func (u *gatewayUpdates) fetch(ctx context.Context, address string) ([]byte, int, error) {
	request, err := http.NewRequestWithContext(ctx, http.MethodGet, address, nil)
	if err != nil {
		return nil, 0, err
	}
	response, err := u.client.Do(request)
	if err != nil {
		return nil, 0, err
	}
	defer response.Body.Close()
	body, err := io.ReadAll(io.LimitReader(response.Body, 4<<20))
	return body, response.StatusCode, err
}

// goProxies are the module proxies go asks, as GOPROXY names them — in the
// environment, or else as go env says: set by go env -w, or go's default —
// but direct and off: those it reaches over HTTP.
func goProxies() []string {
	value := os.Getenv("GOPROXY")
	if value == "" {
		if gobin, err := exec.LookPath("go"); err == nil {
			ctx, cancel := context.WithTimeout(context.Background(), 5*time.Second)
			defer cancel()
			command := exec.CommandContext(ctx, gobin, "env", "GOPROXY")
			command.Dir = os.TempDir() // outside any module
			if output, err := command.Output(); err == nil {
				value = strings.TrimSpace(string(output))
			}
		}
	}
	if value == "" {
		value = "https://proxy.golang.org,direct"
	}
	var proxies []string
	for _, entry := range strings.FieldsFunc(value, func(r rune) bool { return r == ',' || r == '|' }) {
		if entry = strings.TrimSpace(entry); strings.HasPrefix(entry, "https://") || strings.HasPrefix(entry, "http://") {
			proxies = append(proxies, entry)
		}
	}
	return proxies
}

// ---------------------------------------------------------------- the version

// gatewayVersion says which version of the gateway the server runs, which
// the checkout's go.mod requires, and whether a newer release is out;
// refresh asks the module proxies again.
func (s *server) gatewayVersion(ctx context.Context, refresh bool) updateStatus {
	u := s.updates
	found := u.check(ctx, refresh)
	status := updateStatus{
		Module: u.module, Name: moduleName(u.module), Running: u.running,
		Latest: found.latest, Major: found.major, CheckedAt: found.at, Update: u.current(),
	}
	if found.err != nil {
		status.Error = found.err.Error()
	} else if found.at.IsZero() {
		status.Error = "the module proxies did not answer in time"
	}
	replaced := false
	gobin, lookErr := exec.LookPath("go")
	switch {
	case s.rebuild == nil:
		status.Reason = "the server does not build itself from a checkout (it was installed without one, or started with -rebuild=false)"
	case lookErr != nil:
		status.Reason = "there is no go on the PATH to build the server with"
	default:
		status.Checkout = display(s.rebuild.root)
		required, replacement, err := u.requirement(ctx, gobin, s.rebuild.root)
		status.Required, replaced = required, replacement != ""
		switch {
		case err != nil:
			status.Reason = "go.mod cannot be read: " + err.Error()
		case replaced:
			status.Reason = "go.mod replaces " + u.module + " with " + replacement
		case required == "":
			status.Reason = "go.mod does not require " + u.module
		default:
			status.CanUpdate = true
		}
	}
	switch status.Update.State {
	case "updating":
		status.CanUpdate, status.Reason = false, "an update is under way"
	case "updated":
		if s.rebuild != nil {
			build := s.rebuild.current()
			status.Build = &build
		}
	}
	// What a replacement stands for is the user's to move.
	current := cmp.Or(status.Required, status.Running)
	if found.latest != nil && !replaced && validVersion(current) && compareVersions(found.latest.Version, current) > 0 {
		status.Available = true
		for _, v := range found.versions {
			if isRelease(v) && compareVersions(v, current) > 0 && compareVersions(v, found.latest.Version) <= 0 {
				status.Behind++
			}
		}
		status.Changes = compareURL(u.module, current, found.latest.Version)
		status.Command = "go get " + u.module + "@" + found.latest.Version
	}
	return status
}

// requirement reads which version of the gateway the checkout's go.mod
// requires, and what replaces it there, if anything does.
func (u *gatewayUpdates) requirement(ctx context.Context, gobin, root string) (version, replacement string, err error) {
	ctx, cancel := context.WithTimeout(ctx, 10*time.Second)
	defer cancel()
	output, err := u.goCommand(ctx, gobin, root, "mod", "edit", "-json").Output()
	if err != nil {
		var exit *exec.ExitError
		if errors.As(err, &exit) && len(exit.Stderr) > 0 {
			return "", "", errors.New(strings.TrimSpace(string(exit.Stderr)))
		}
		return "", "", err
	}
	var file struct {
		Require []struct{ Path, Version string }
		Replace []struct {
			Old, New struct{ Path, Version string }
		}
	}
	if err := json.Unmarshal(output, &file); err != nil {
		return "", "", err
	}
	for _, r := range file.Require {
		if r.Path == u.module {
			version = r.Version
		}
	}
	for _, r := range file.Replace {
		if r.Old.Path == u.module && (r.Old.Version == "" || r.Old.Version == version) {
			replacement = strings.TrimSpace(r.New.Path + " " + r.New.Version)
		}
	}
	return version, replacement, nil
}

// ---------------------------------------------------------------- the update

func (u *gatewayUpdates) current() updateJob {
	u.mu.Lock()
	defer u.mu.Unlock()
	return u.job
}

// setJob records what the update does now, and says it in the terminal.
func (u *gatewayUpdates) setJob(job updateJob) {
	job.At = time.Now()
	u.mu.Lock()
	u.job = job
	u.mu.Unlock()
	u.say(job)
}

// begin records an update that starts, unless one is under way.
func (u *gatewayUpdates) begin(job updateJob) bool {
	job.At = time.Now()
	u.mu.Lock()
	if u.job.State == "updating" {
		u.mu.Unlock()
		return false
	}
	u.job = job
	u.mu.Unlock()
	u.say(job)
	return true
}

func (u *gatewayUpdates) say(job updateJob) {
	if job.Message != "" {
		fmt.Printf("%s update %s\n", job.At.Format("15:04:05"), job.Message)
	}
	if job.State == "failed" && job.Output != "" {
		fmt.Println(indent(job.Output))
	}
}

// startUpdate starts taking up version of the gateway, which must be the
// latest release the check found: the one the page offered.
func (s *server) startUpdate(ctx context.Context, version string) (updateStatus, error) {
	status := s.gatewayVersion(ctx, false)
	switch {
	case status.Update.State == "updating":
		return status, errors.New("an update is under way")
	case !status.CanUpdate:
		return status, errors.New("the server cannot update the gateway itself: " + status.Reason)
	case status.Latest == nil:
		return status, errors.New("the gateway's latest release is not known: " + cmp.Or(status.Error, "check again"))
	case version != status.Latest.Version:
		return status, fmt.Errorf("%s is not the latest release of %s: %s is", version, status.Name, status.Latest.Version)
	case !status.Available:
		return status, fmt.Errorf("go.mod requires %s %s already", status.Name, status.Required)
	}
	gobin, err := exec.LookPath("go")
	if err != nil {
		return status, err
	}
	u := s.updates
	if !u.begin(updateJob{State: "updating", Version: version, Message: fmt.Sprintf("fetching %s %s", status.Name, version)}) {
		return status, errors.New("an update is under way")
	}
	go s.update(u.life, s.rebuild.root, gobin, version)
	status.Update, status.CanUpdate, status.Reason = u.current(), false, "an update is under way"
	return status, nil
}

// update takes version of the gateway up in the checkout at root: go get
// moves copies of go.mod and go.sum to it, the programs the server builds
// (rebuild.go) are built with the copies, and only once they build do the
// copies take the place of go.mod and go.sum.
func (s *server) update(ctx context.Context, root, gobin, version string) {
	u := s.updates
	name, target := moduleName(u.module), u.module+"@"+version
	fail := func(message, output string) {
		u.setJob(updateJob{State: "failed", Version: version, Message: message + "; go.mod and go.sum stay as they were", Output: strings.TrimSpace(lastLines(output, 30))})
	}
	temporary, err := os.MkdirTemp("", "kou-conveyor-update-")
	if err != nil {
		fail("cannot update: "+err.Error(), "")
		return
	}
	defer os.RemoveAll(temporary)
	modPath, sumPath := filepath.Join(root, "go.mod"), filepath.Join(root, "go.sum")
	mod, err := os.ReadFile(modPath)
	if err != nil {
		fail("cannot read go.mod: "+err.Error(), "")
		return
	}
	sum, err := os.ReadFile(sumPath)
	if err != nil && !errors.Is(err, fs.ErrNotExist) {
		fail("cannot read go.sum: "+err.Error(), "")
		return
	}
	modCopy, sumCopy := filepath.Join(temporary, "go.mod"), filepath.Join(temporary, "go.sum")
	if err := errors.Join(os.WriteFile(modCopy, mod, 0o644), os.WriteFile(sumCopy, sum, 0o644)); err != nil {
		fail("cannot update: "+err.Error(), "")
		return
	}
	fetched, err := u.run(ctx, updateFetchTime, gobin, root, "get", "-modfile="+modCopy, target)
	if err != nil {
		fail(failure("go get "+target+" failed", err), fetched)
		return
	}
	u.setJob(updateJob{State: "updating", Version: version, Message: fmt.Sprintf("building the programs with %s %s", name, version)})
	build := append([]string{"build", "-trimpath", "-mod=readonly", "-modfile=" + modCopy, "-o", temporary + string(filepath.Separator)}, s.programPackages()...)
	if built, err := u.run(ctx, updateBuildTime, gobin, root, build...); err != nil {
		fail(failure(fmt.Sprintf("the programs do not build with %s %s", name, version), err), built)
		return
	}
	// Whoever changed go.mod or go.sum meanwhile has the last word.
	if !unchanged(modPath, mod) || !unchanged(sumPath, sum) {
		fail("go.mod or go.sum changed while the update ran", "")
		return
	}
	newMod, err := os.ReadFile(modCopy)
	if err != nil {
		fail("cannot update: "+err.Error(), "")
		return
	}
	newSum, err := os.ReadFile(sumCopy)
	if err != nil {
		fail("cannot update: "+err.Error(), "")
		return
	}
	// go.sum first: a build in between has the old go.mod, with sums to
	// spare.
	if err := replaceFile(sumPath, newSum); err != nil {
		fail("cannot write go.sum: "+err.Error(), "")
		return
	}
	if err := replaceFile(modPath, newMod); err != nil {
		if sum != nil {
			replaceFile(sumPath, sum)
		} else {
			os.Remove(sumPath)
		}
		fail("cannot write go.mod: "+err.Error(), "")
		return
	}
	u.setJob(updateJob{
		State: "updated", Version: version, Output: changes(fetched),
		Message: fmt.Sprintf("go.mod requires %s %s now: the server builds itself anew, and restarts with it once no agent is at work", name, version),
	})
}

// programPackages are the packages of the programs the server builds when
// its code changes (rebuild.go): itself, and those beside it.
func (s *server) programPackages() []string {
	var packages []string
	for _, name := range programs {
		if name != "kou-conveyor-web" {
			if _, err := os.Stat(filepath.Join(s.rebuild.dir, name)); err != nil {
				continue
			}
		}
		packages = append(packages, "./cmd/"+name)
	}
	return packages
}

// goCommand is a go command that runs in dir, outside any go.work.
func (u *gatewayUpdates) goCommand(ctx context.Context, gobin, dir string, args ...string) *exec.Cmd {
	command := exec.CommandContext(ctx, gobin, args...)
	command.Dir = dir
	command.Env = append(append(os.Environ(), "GOWORK=off"), u.env...)
	command.WaitDelay = 5 * time.Second
	return command
}

// run runs a go command in dir for at most limit, and returns what it said.
func (u *gatewayUpdates) run(ctx context.Context, limit time.Duration, gobin, dir string, args ...string) (string, error) {
	ctx, cancel := context.WithTimeout(ctx, limit)
	defer cancel()
	output, err := u.goCommand(ctx, gobin, dir, args...).CombinedOutput()
	if err != nil && errors.Is(ctx.Err(), context.DeadlineExceeded) {
		err = fmt.Errorf("it took longer than %s", limit)
	}
	return string(output), err
}

// failure says what failed, and why when go's output does not.
func failure(what string, err error) string {
	var exit *exec.ExitError
	if errors.As(err, &exit) {
		return what
	}
	return what + ": " + err.Error()
}

// unchanged reports whether a file still holds what it held, or still is
// not there.
func unchanged(path string, was []byte) bool {
	now, err := os.ReadFile(path)
	if errors.Is(err, fs.ErrNotExist) {
		return was == nil
	}
	return err == nil && was != nil && bytes.Equal(now, was)
}

// replaceFile puts content in place of a file's at once: written beside it,
// then renamed over it, keeping its permissions.
func replaceFile(path string, content []byte) error {
	mode := fs.FileMode(0o644)
	if info, err := os.Stat(path); err == nil {
		mode = info.Mode().Perm()
	}
	temporary := filepath.Join(filepath.Dir(path), "."+filepath.Base(path)+".update")
	os.Remove(temporary)
	if err := os.WriteFile(temporary, content, mode); err != nil {
		os.Remove(temporary)
		return err
	}
	if err := os.Rename(temporary, path); err != nil {
		os.Remove(temporary)
		return err
	}
	return nil
}

// changes are the lines of go get's output that say what it moved.
func changes(output string) string {
	var moved []string
	for line := range strings.Lines(output) {
		line = strings.TrimSpace(strings.TrimPrefix(line, "go: "))
		for _, verb := range []string{"upgraded ", "downgraded ", "added ", "removed "} {
			if strings.HasPrefix(line, verb) {
				moved = append(moved, line)
				break
			}
		}
	}
	return strings.Join(moved, "\n")
}

// ---------------------------------------------------------------- requests

// handleGatewayVersion answers which version of the gateway the server
// runs, which go.mod requires, whether a newer release is out, and how an
// update goes; ?refresh=1 asks the module proxies again.
func (s *server) handleGatewayVersion(w http.ResponseWriter, r *http.Request) {
	writeJSON(w, http.StatusOK, s.gatewayVersion(r.Context(), r.URL.Query().Get("refresh") != ""))
}

// handleUpdateGateway starts taking up the gateway's latest release, which
// {"version"} names: the one the page offered.
func (s *server) handleUpdateGateway(w http.ResponseWriter, r *http.Request) {
	var body struct {
		Version string `json:"version"`
	}
	if !decodeJSON(w, r, &body) {
		return
	}
	if !validVersion(body.Version) {
		writeError(w, http.StatusBadRequest, "version is a module version, such as v8.0.10")
		return
	}
	status, err := s.startUpdate(r.Context(), body.Version)
	if err != nil {
		writeJSON(w, http.StatusConflict, map[string]any{"error": err.Error(), "status": status})
		return
	}
	writeJSON(w, http.StatusAccepted, status)
}

// ---------------------------------------------------------------- versions and paths

// semver is a module version: vMAJOR.MINOR.PATCH, perhaps with a
// -pre.release.
type semver struct {
	major, minor, patch int
	pre                 []string
}

// parseVersion parses a module version. One with build metadata, such as
// +incompatible, is no version of a module of its own.
func parseVersion(v string) (semver, bool) {
	rest, ok := strings.CutPrefix(v, "v")
	if !ok {
		return semver{}, false
	}
	core, pre, hasPre := strings.Cut(rest, "-")
	numbers := strings.Split(core, ".")
	if len(numbers) != 3 {
		return semver{}, false
	}
	var parsed semver
	for i, to := range []*int{&parsed.major, &parsed.minor, &parsed.patch} {
		n, ok := versionNumber(numbers[i])
		if !ok {
			return semver{}, false
		}
		*to = n
	}
	if hasPre {
		for _, id := range strings.Split(pre, ".") {
			if id == "" || strings.TrimLeft(id, "0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ-") != "" {
				return semver{}, false
			}
			if numeric(id) && len(id) > 1 && id[0] == '0' {
				return semver{}, false
			}
			parsed.pre = append(parsed.pre, id)
		}
	}
	return parsed, true
}

// versionNumber parses a number of a version: digits, without a leading
// zero.
func versionNumber(s string) (int, bool) {
	if !numeric(s) || len(s) > 9 || len(s) > 1 && s[0] == '0' {
		return 0, false
	}
	n := 0
	for _, r := range s {
		n = n*10 + int(r-'0')
	}
	return n, true
}

func numeric(s string) bool { return s != "" && strings.Trim(s, "0123456789") == "" }

func validVersion(v string) bool {
	_, ok := parseVersion(v)
	return ok
}

// isRelease reports whether v is a release: a version, but no pre-release.
func isRelease(v string) bool {
	parsed, ok := parseVersion(v)
	return ok && len(parsed.pre) == 0
}

// compareVersions orders module versions by semantic versioning: -1, 0 or
// +1. What is no version comes before every version.
func compareVersions(a, b string) int {
	x, okA := parseVersion(a)
	y, okB := parseVersion(b)
	switch {
	case okA && !okB:
		return 1
	case !okA && okB:
		return -1
	case !okA:
		return strings.Compare(a, b)
	}
	if c := cmp.Compare(x.major, y.major); c != 0 {
		return c
	}
	if c := cmp.Compare(x.minor, y.minor); c != 0 {
		return c
	}
	if c := cmp.Compare(x.patch, y.patch); c != 0 {
		return c
	}
	switch {
	case len(x.pre) == 0 && len(y.pre) == 0:
		return 0
	case len(x.pre) == 0:
		return 1
	case len(y.pre) == 0:
		return -1
	}
	for i := range min(len(x.pre), len(y.pre)) {
		a, b := x.pre[i], y.pre[i]
		var c int
		switch {
		case numeric(a) && numeric(b):
			c = cmp.Or(cmp.Compare(len(a), len(b)), strings.Compare(a, b))
		case numeric(a):
			c = -1
		case numeric(b):
			c = 1
		default:
			c = strings.Compare(a, b)
		}
		if c != 0 {
			return c
		}
	}
	return cmp.Compare(len(x.pre), len(y.pre))
}

// highest is the highest of versions, or of their releases.
func highest(versions []string, releases bool) string {
	best := ""
	for _, v := range versions {
		if (!releases || isRelease(v)) && (best == "" || compareVersions(v, best) > 0) {
			best = v
		}
	}
	return best
}

// ofModule reports whether v is a version of module: of the major version
// its path ends with, or v0 or v1 when it ends with none.
func ofModule(module, v string) bool {
	parsed, ok := parseVersion(v)
	if !ok {
		return false
	}
	if _, major := splitMajor(module); major != 0 {
		return parsed.major == major
	}
	return parsed.major <= 1
}

// escapeModulePath escapes a module path, or a version, for a module
// proxy: an upper-case letter becomes ! and the letter in lower case.
func escapeModulePath(s string) string {
	var escaped strings.Builder
	for _, r := range s {
		if 'A' <= r && r <= 'Z' {
			escaped.WriteByte('!')
			r += 'a' - 'A'
		}
		escaped.WriteRune(r)
	}
	return escaped.String()
}

// splitMajor splits a module path into the path before its major version
// and the major version: the /vN it ends with, N ≥ 2, or 0 when it ends
// with none.
func splitMajor(module string) (string, int) {
	i := strings.LastIndex(module, "/v")
	if i < 0 {
		return module, 0
	}
	n, ok := versionNumber(module[i+2:])
	if !ok || n < 2 {
		return module, 0
	}
	return module[:i], n
}

// moduleName is the name a module goes by: its path's last element but the
// major version, CLIProxyAPI for github.com/router-for-me/CLIProxyAPI/v8.
func moduleName(module string) string {
	prefix, _ := splitMajor(module)
	return path.Base(prefix)
}

// repository is the GitHub repository whose tags are a module's versions,
// for a module at the root of one.
func repository(module string) string {
	prefix, _ := splitMajor(module)
	if parts := strings.Split(prefix, "/"); len(parts) == 3 && parts[0] == "github.com" {
		return "https://" + prefix
	}
	return ""
}

// releaseURL is the page of a release of a module on GitHub.
func releaseURL(module, version string) string {
	if repo := repository(module); repo != "" {
		return repo + "/releases/tag/" + version
	}
	return ""
}

// compareURL is the page of what changed between two releases of a module
// on GitHub.
func compareURL(module, from, to string) string {
	if repo := repository(module); repo != "" && isRelease(from) && isRelease(to) {
		return repo + "/compare/" + from + "..." + to
	}
	return ""
}
