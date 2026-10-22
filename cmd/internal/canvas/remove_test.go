package canvas

import (
	"errors"
	"io/fs"
	"net/http"
	"os"
	"testing"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// ranSession gives a session its file, as its first run does.
func ranSession(t *testing.T, te *testEngine, session string) {
	t.Helper()
	dir := te.host.ws.SessionDir
	if err := os.MkdirAll(dir, 0o700); err != nil {
		t.Fatal(err)
	}
	if err := os.WriteFile(cockpit.SessionPath(dir, session), nil, 0o600); err != nil {
		t.Fatal(err)
	}
}

// gone reports whether a session's file is no more.
func gone(te *testEngine, session string) bool {
	_, err := os.Stat(cockpit.SessionPath(te.host.ws.SessionDir, session))
	return errors.Is(err, fs.ErrNotExist)
}

// A kou agent's session goes with its node unless asked to stay: the
// session list never shows an agent that was removed.
func TestARemovedAgentsSessionGoesWithIt(t *testing.T) {
	te := newEngine(t)
	dir := te.host.ws.SessionDir
	id, c := te.newCanvas(t, "Removal")
	res := te.ops(t, id, `[
		{"op":"node.add","node":{"preset":"agent","title":"Idle"}},
		{"op":"node.add","node":{"preset":"agent","title":"Busy"}},
		{"op":"node.add","node":{"preset":"agent","title":"Locked"}},
		{"op":"node.add","node":{"preset":"agent","title":"Kept"}}
	]`)
	idle, busy, locked, kept := res.IDs["0"], res.IDs["1"], res.IDs["2"], res.IDs["3"]
	sIdle, sBusy, sLocked, sKept := sessionOf(t, c, idle), sessionOf(t, c, busy), sessionOf(t, c, locked), sessionOf(t, c, kept)
	for _, s := range []string{sIdle, sBusy, sLocked, sKept} {
		ranSession(t, te, s)
	}
	// One still runs; the runner of another lets go of it a moment later.
	te.host.setRunning(sBusy, true)
	unlock, err := cockpit.LockSession(dir, sLocked)
	if err != nil {
		t.Fatal(err)
	}
	time.AfterFunc(400*time.Millisecond, unlock)

	// Removed as the user's delete does, with no word of keeping, the
	// sessions go too: a run still going is stopped first.
	te.ops(t, id, `[
		{"op":"node.remove","id":"`+idle+`","grace_s":0},
		{"op":"node.remove","id":"`+busy+`","grace_s":0},
		{"op":"node.remove","id":"`+locked+`","grace_s":0}
	]`)
	for _, s := range []string{sIdle, sBusy, sLocked} {
		eventually(t, 5*time.Second, "the session "+s+" to go", func() bool { return gone(te, s) })
		if meta := cockpit.LoadMeta(dir, s); meta.Canvas != "" {
			t.Fatalf("the metadata of a session deleted: %+v", meta)
		}
	}
	if te.host.RunActive(te.host.ws, sBusy) {
		t.Fatal("the run of a node removed goes on")
	}
	sessions, err := cockpit.ListSessions(dir)
	if err != nil || len(sessions) != 1 || sessions[0].ID != sKept {
		t.Fatalf("the session list: %+v, %v", sessions, err)
	}

	// Asked to, a node leaves its session to the session list.
	te.ops(t, id, `[{"op":"node.remove","id":"`+kept+`","keep":{"session":true},"grace_s":0}]`)
	eventually(t, 5*time.Second, "the session to be let go", func() bool { return cockpit.LoadMeta(dir, sKept).Canvas == "" })
	if gone(te, sKept) {
		t.Fatal("a session kept went with its node")
	}

	// A canvas deleted takes its agents' sessions with it, their runs
	// stopped; with keep_sessions it leaves them.
	other, oc := te.newCanvas(t, "Deleted")
	res = te.ops(t, other, `[{"op":"node.add","node":{"preset":"agent","title":"Worker"}}]`)
	sWorker := sessionOf(t, oc, res.IDs["0"])
	ranSession(t, te, sWorker)
	te.host.setRunning(sWorker, true)
	te.must(t, http.StatusNoContent, "DELETE", "/api/w/w1/canvases/"+other, "", "", nil)
	if !gone(te, sWorker) || te.host.RunActive(te.host.ws, sWorker) {
		t.Fatalf("the session of a canvas deleted: gone %v, running %v", gone(te, sWorker), te.host.RunActive(te.host.ws, sWorker))
	}
	third, tc := te.newCanvas(t, "Deleted, sessions kept")
	res = te.ops(t, third, `[{"op":"node.add","node":{"preset":"agent","title":"Worker"}}]`)
	sStays := sessionOf(t, tc, res.IDs["0"])
	ranSession(t, te, sStays)
	te.must(t, http.StatusNoContent, "DELETE", "/api/w/w1/canvases/"+third+"?keep_sessions=1", "", "", nil)
	if gone(te, sStays) || cockpit.LoadMeta(dir, sStays).Canvas != "" {
		t.Fatalf("a session kept with its canvas deleted: gone %v, %+v", gone(te, sStays), cockpit.LoadMeta(dir, sStays))
	}
}
