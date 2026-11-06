package coordinator

import (
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
)

func TestCoordinatorCheckpointSurvivesWarmArchivalCache(t *testing.T) {
	dir, _ := checkpointBenchmarkFixture(t, 2, 100)
	store, err := localfile.New(dir)
	checkpointRequire(t, err)
	store.MapLogs()
	first, err := store.Resume(t.Context(), checkpointSession)
	checkpointRequire(t, err)
	if first.Checkpoint == nil {
		t.Fatal("missing initial checkpoint")
	}
	page, err := store.Items(t.Context(), checkpointSession, sessionstore.BeforeFirst, 100)
	checkpointRequire(t, err)
	if len(page.Items) <= len(first.Checkpoint.Items) {
		t.Fatal("archival upgrade did not return older history")
	}
	_, err = store.Inspect(t.Context(), checkpointSession)
	checkpointRequire(t, err)
	_, err = store.Fork(t.Context(), "warm-fork", checkpointSession, "turn-0")
	checkpointRequire(t, err)
	resumed, err := store.Resume(t.Context(), checkpointSession)
	checkpointRequire(t, err)
	if resumed.Checkpoint == nil || resumed.Checkpoint.After != first.Checkpoint.After {
		t.Fatal("full-cache upgrade disabled checkpoint resume")
	}
	counted := &countedHistory{Store: store}
	current := checkpointCoordinator(counted, resumed)
	checkpointRequire(t, current.restore(t.Context()))
	if counted.reads != 0 {
		t.Fatalf("warm resume replayed %d archival pages", counted.reads)
	}
}
