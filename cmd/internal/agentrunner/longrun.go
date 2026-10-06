package agentrunner

import (
	"bufio"
	"encoding/json/v2"
	"errors"
	"fmt"
	"io/fs"
	"os"
	"path/filepath"
	"strconv"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/coordinator"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
)

// Long runs. A run that goes on for hours compacts its conversation again
// and again, and every compaction summarizes the summary before it: what one
// leaves out, no later one gets back. Next to the session's file the run
// keeps two more of the session's own (contextbuilder.Memory): the notes the
// model writes for itself, which every compaction shows and none changes,
// and the summaries of the session's compactions, oldest first. When a
// cockpit records how the session changes the workspace (cockpit.Changes),
// a compaction tells the model the snapshot of the workspace it came at, to
// see what changed since. The environment bounds the run's turns, time and
// compactions, and says how often the model reports how the work goes.

const (
	maxTurnsEnvironment       = "KOU_CONVEYOR_MAX_TURNS"
	maxDurationEnvironment    = "KOU_CONVEYOR_MAX_DURATION"
	maxCompactionsEnvironment = "KOU_CONVEYOR_MAX_COMPACTIONS"
	reportEveryEnvironment    = "KOU_CONVEYOR_REPORT_EVERY"
	// defaultReportEvery is how often a run asks the model how the work
	// goes.
	defaultReportEvery = 30 * time.Minute
	// stuckCompactions is how many compactions in a row with no file changed
	// have the harness ask the model whether it goes round in circles.
	stuckCompactions = 3
)

// runBounds bound a run (coordinator.Dependencies); zero sets no bound.
type runBounds struct {
	maxTurns, maxCompactions int
	maxDuration, reportEvery time.Duration
}

// readRunBounds reads the bounds of a run from the environment.
func readRunBounds(getenv func(string) string) (runBounds, error) {
	bounds := runBounds{reportEvery: defaultReportEvery}
	for _, count := range []struct {
		name  string
		value *int
	}{{maxTurnsEnvironment, &bounds.maxTurns}, {maxCompactionsEnvironment, &bounds.maxCompactions}} {
		if setting := strings.TrimSpace(getenv(count.name)); setting != "" {
			parsed, err := strconv.Atoi(setting)
			if err != nil || parsed < 0 {
				return runBounds{}, fmt.Errorf("%s must be a whole number, or 0 for no limit", count.name)
			}
			*count.value = parsed
		}
	}
	for _, duration := range []struct {
		name  string
		value *time.Duration
	}{{maxDurationEnvironment, &bounds.maxDuration}, {reportEveryEnvironment, &bounds.reportEvery}} {
		switch setting := strings.ToLower(strings.TrimSpace(getenv(duration.name))); setting {
		case "":
		case "0", "off":
			*duration.value = 0
		default:
			parsed, err := time.ParseDuration(setting)
			if err != nil || parsed <= 0 {
				return runBounds{}, fmt.Errorf("%s must be a duration such as 90m or 8h, or 0 for none", duration.name)
			}
			*duration.value = parsed
		}
	}
	return bounds, nil
}

// sessionMemory is what a session keeps outside its conversation.
type sessionMemory struct {
	// notes and summaries are the session's files of notes and summaries.
	notes, summaries string
	// records and repository are where a cockpit records the snapshots of
	// the workspace the session's runs saw, a line of JSON each, whose tree
	// is in the repository.
	records, repository string
	// fileTools reports a run offered Edit, Write or apply_patch, whose
	// calls count as changes (coordinator.Compaction).
	fileTools bool
	// checkpoint is the snapshot the run's latest compaction came at; idle
	// counts the compactions in a row with no file changed.
	checkpoint string
	idle       int
}

func newSessionMemory(directory string, id session.ID, fileTools bool) *sessionMemory {
	changes := filepath.Join(directory, ".changes")
	return &sessionMemory{
		notes:      filepath.Join(directory, string(id)+".notes.md"),
		summaries:  filepath.Join(directory, string(id)+".summaries.md"),
		records:    filepath.Join(changes, string(id)+".jsonl"),
		repository: filepath.Join(changes, "repo.git"),
		fileTools:  fileTools,
	}
}

// builder is what the context builder shows of the memory.
func (memory *sessionMemory) builder() contextbuilder.Memory {
	return contextbuilder.Memory{
		Notes: memory.notes,
		ReadNotes: func() (string, error) {
			data, err := os.ReadFile(memory.notes)
			if errors.Is(err, fs.ErrNotExist) {
				return "", nil
			}
			return string(data), err
		},
		Summaries: memory.summaries,
	}
}

// reminder is what the model is told as the conversation nears its
// compaction.
func (memory *sessionMemory) reminder() string {
	return "Bring your notes in " + memory.notes + " up to date before the summary replaces the conversation: the hypotheses you are testing, the evidence for and against them, the negative results, what you deferred and the next step. The summary may lose them; the notes follow it as they are."
}

// compacted keeps the summary of a compaction of the run, and returns what
// the model is told of it: the snapshot of the workspace the compaction came
// at, and whether the work seems to go round in circles.
func (memory *sessionMemory) compacted(compaction coordinator.Compaction, now time.Time) (string, error) {
	var notes []string
	before, tree := memory.snapshots()
	since := "the prompt's run began"
	if memory.checkpoint != "" {
		before, since = memory.checkpoint, "the compaction before"
	}
	header := "## Compaction at " + now.UTC().Format(time.RFC3339)
	progress := compaction.FileChanges > 0
	if tree != "" {
		header += ", workspace snapshot " + tree
		if before != "" && before != tree {
			progress = true
			notes = append(notes, fmt.Sprintf("The workspace at this compaction is snapshot %s in the git repository %s: `git --git-dir %s diff --stat %s %s` shows what changed since %s, and `-- <path>` in place of --stat a file's diff.",
				tree, memory.repository, memory.repository, before, tree, since))
		}
		memory.checkpoint = tree
	}
	err := appendText(memory.summaries, header+"\n\n"+strings.TrimSpace(compaction.Summary)+"\n\n")
	if progress || !memory.fileTools && tree == "" {
		memory.idle = 0
	} else if memory.idle++; memory.idle >= stuckCompactions {
		notes = append(notes, fmt.Sprintf("%d compactions in a row came with no file changed. If the task is to change files and the work goes round in circles, the same files read again, the same commands, the same hypotheses: stop, write in your notes what you tried and what it showed, then change the approach, or tell the user what blocks you.", memory.idle))
		memory.idle = 0
	}
	return strings.Join(notes, "\n\n"), err
}

// snapshots reads the cockpit's records of the session's snapshots of the
// workspace: the latest taken as a prompt's run started, and the latest.
func (memory *sessionMemory) snapshots() (before, latest string) {
	file, err := os.Open(memory.records)
	if err != nil {
		return "", ""
	}
	defer file.Close()
	scanner := bufio.NewScanner(file)
	scanner.Buffer(make([]byte, 0, 64<<10), 4<<20)
	for scanner.Scan() {
		var record struct {
			Tree   string `json:"tree"`
			Before bool   `json:"before"`
		}
		if json.Unmarshal(scanner.Bytes(), &record) != nil || record.Tree == "" {
			continue
		}
		if record.Before {
			before = record.Tree
		}
		latest = record.Tree
	}
	return before, latest
}

// appendText adds text to the end of a file, which it creates if need be.
func appendText(path, text string) error {
	file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return err
	}
	_, err = file.WriteString(text)
	return errors.Join(err, file.Close())
}
