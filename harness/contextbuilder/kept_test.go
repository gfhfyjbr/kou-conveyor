package contextbuilder

import (
	"fmt"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
)

// A run that restores a compacted session shows the compaction as the run
// that made it did, though the notes changed since: the start of every
// request stays the same, and with it what the provider cached. What
// changed in the notes follows, after the cached part.
func TestRestoredCompactionShowsTheNotesAsTheyWere(t *testing.T) {
	notes := "- hypothesis A"
	memory := Memory{
		Notes:     "/work/.harness/sessions/abc.notes.md",
		ReadNotes: func() (string, error) { return notes, nil },
	}
	conversation := func() Builder {
		current := NewBuilder()
		current.SetMemory(memory)
		addInput(t, current, "task")
		current.Commit()
		current.AddModelResponse(answer("done"))
		current.Commit()
		return current
	}
	summary := answer("<summary>The work so far.</summary>")

	live := conversation()
	made, ok := live.Compact(summary, nil)
	if !ok || made.Notes == nil || *made.Notes != "- hypothesis A" || !strings.Contains(made.Message, "<notes>\n- hypothesis A\n</notes>") {
		t.Fatalf("made = %+v, %v", made, ok)
	}
	if _, changed := live.NotesUpdate(); changed {
		t.Fatal("notes the compaction just showed are not news")
	}

	// The next run restores the session after the notes changed.
	notes = "- hypothesis A: refuted\n- hypothesis B"
	restored := conversation()
	if _, ok := restored.Compact(summary, &made); !ok {
		t.Fatal("did not compact")
	}
	built, err := restored.Build()
	if err != nil {
		t.Fatal(err)
	}
	if messageText(built.Request.Input[1]) != made.Message {
		t.Fatalf("restored compaction differs:\n%s", messageText(built.Request.Input[1]))
	}
	update, changed := restored.NotesUpdate()
	if !changed || !strings.HasPrefix(update, NotesUpdatePrefix) || !strings.Contains(update, "<notes>\n- hypothesis A: refuted\n- hypothesis B\n</notes>") || !strings.Contains(update, "abc.notes.md") {
		t.Fatalf("update = %q, %v", update, changed)
	}
	// Once the conversation shows the update, it is no news either; and
	// a run that restores it, with the update among its messages, knows so.
	restored.AddControlMessage(inbox.ControlMessage{Mode: inbox.Heartbeat, Reason: update})
	if _, changed := restored.NotesUpdate(); changed {
		t.Fatal("the update shown is news again")
	}

	// A compaction recorded before the message was kept is made anew.
	old := conversation()
	again, ok := old.Compact(summary, nil)
	if !ok || !strings.Contains(again.Message, "- hypothesis B") {
		t.Fatalf("made anew = %+v", again)
	}
	if _, changed := old.NotesUpdate(); changed {
		t.Fatal("notes made anew are not news")
	}

	// Emptied notes say so.
	notes = ""
	if update, changed := restored.NotesUpdate(); !changed || !strings.Contains(update, "is empty now") {
		t.Fatalf("update = %q, %v", update, changed)
	}
}

// Notes past NotesLimit show their beginning and end and say, between
// them, how much is left out and where the whole file is.
func TestLongNotesSayWhatIsLeftOut(t *testing.T) {
	var long strings.Builder
	for line := 0; long.Len() < 3*NotesLimit; line++ {
		long.WriteString("- note " + strings.Repeat("x", 60) + " ✓\n")
	}
	shown := ShownNotes("- FIRST\n"+long.String()+"- LAST", "/work/n.md")
	if len(shown) > NotesLimit+300 || !strings.HasPrefix(shown, "- FIRST\n") || !strings.HasSuffix(shown, "- LAST") {
		t.Fatalf("shown %d bytes:\n%.200s … %.200s", len(shown), shown, shown[len(shown)-200:])
	}
	if !strings.Contains(shown, "KB from the middle of the notes are left out here, of 96 KB in all: the whole file is /work/n.md") {
		t.Fatalf("no mark of what is left out:\n%s", shown[NotesLimit*2/3-200:NotesLimit*2/3+400])
	}
	// The cuts fall at the ends of lines.
	at := strings.Index(shown, "\n[… ")
	if shown[at-1] != '\n' && !strings.HasSuffix(shown[:at], "✓") {
		t.Fatalf("cut within a line: %q", shown[at-40:at+10])
	}
	if short := ShownNotes("  - short\n", "/work/n.md"); short != "- short" {
		t.Fatalf("short = %q", short)
	}
}

// The compaction prompt says how much of the notes the harness shows.
func TestCompactionPromptSaysHowMuchOfTheNotesIsShown(t *testing.T) {
	if want := fmt.Sprintf("up to %d KB", NotesLimit/1000); !strings.Contains(compactionPrompt, want) {
		t.Fatalf("compaction prompt lacks %q", want)
	}
}
