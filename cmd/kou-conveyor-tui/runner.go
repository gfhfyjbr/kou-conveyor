package main

import (
	"context"
	"errors"
	"fmt"
	"io"
	"strings"
	"time"
	"unicode"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/cockpit"
)

// runHeadless runs one prompt without a terminal: answers go to out,
// diagnostics and errors to diagnostics. "/compact [focus]" compacts the
// session instead, and its summary is the answer.
func runHeadless(ctx context.Context, o options, out, diagnostics io.Writer) error {
	id := o.session
	focus, compact := compactCommand(o.prompt)
	if compact && id == "" {
		return errors.New("-p /compact needs the session to compact: add -session <id> or -continue")
	}
	if id == "" {
		id = uuid.New().String()
	}
	request := cockpit.Request{SessionID: id, Model: o.model, Thinking: o.thinking}
	// A session goes on with the model its last prompt ran with, unless
	// -model names another.
	if request.Model == "" && o.session != "" {
		if tr, err := cockpit.LoadSession(o.SessionDir, id); err == nil {
			request.Model = tr.LastModel()
		}
	}
	if compact {
		request.Compact, request.Instructions = true, focus
	} else {
		request.MessageID, request.Prompt = uuid.New().String(), o.prompt
		if history, err := loadHistory(o.historyFile); err != nil {
			fmt.Fprintln(diagnostics, err)
		} else if _, err := saveHistory(o.historyFile, appendHistory(history, o.prompt, id, o.model)); err != nil {
			fmt.Fprintln(diagnostics, err)
		}
	}
	// What the run changes is recorded, to be seen in either cockpit later.
	var tracker *cockpit.Tracker
	if !compact {
		snapshot, cancel := context.WithTimeout(ctx, 5*time.Second)
		tracker, _ = cockpit.NewChanges(o.Workspace, o.SessionDir, o.LogDir).Begin(snapshot, id, request.MessageID)
		cancel()
	}
	job, err := cockpit.Start(ctx, o.Options, request)
	if err != nil && tracker != nil {
		tracker.Cancel()
	}
	if compact && errors.Is(err, cockpit.ErrSessionGone) {
		return fmt.Errorf("session %s does not exist, so there is nothing to compact", id)
	}
	if err != nil {
		return err
	}
	defer func() {
		job.Cancel()
		for range job.Lines() {
		}
		if tracker != nil {
			// The last snapshot: what the run left.
			tracker.Finish()
			wait := time.After(10 * time.Second)
			for {
				select {
				case _, ok := <-tracker.Updates():
					if !ok {
						return
					}
				case <-wait:
					return
				}
			}
		}
	}()
	tr := cockpit.NewTranscript()
	if compact {
		tr.Begin(uuid.New().String(), "Compacting context")
	} else {
		tr.Submit(request.MessageID, o.prompt, time.Now().UTC())
	}
	printed := make(map[string]bool)
	compacted := false
	var failure error
	report := func(entries []*cockpit.Entry) error {
		for _, e := range entries {
			if printed[e.ID] {
				continue
			}
			switch e.Kind {
			case cockpit.KindAssistant:
				printed[e.ID] = true
				if _, err := fmt.Fprintln(out, e.Text); err != nil {
					return err
				}
			case cockpit.KindError:
				printed[e.ID] = true
				failure = errors.Join(failure, errors.New(e.Text))
				fmt.Fprintln(diagnostics, e.Text)
			case cockpit.KindNotice:
				if !strings.HasPrefix(e.ID, "compaction:") {
					continue
				}
				// Automatic compactions are worth a line; the one asked
				// for answers with its summary.
				printed[e.ID], compacted = true, true
				fmt.Fprintln(diagnostics, e.Text)
				if compact && e.Detail != "" {
					if _, err := fmt.Fprintln(out, e.Detail); err != nil {
						return err
					}
				}
			}
		}
		return nil
	}
	for line := range job.Lines() {
		if line.Stderr {
			fmt.Fprintln(diagnostics, cockpit.Clean(line.Text))
			continue
		}
		changed, err := tr.Apply([]byte(line.Text))
		if err != nil {
			return err
		}
		for _, e := range changed {
			if e.Kind == cockpit.KindTool && e.Tool.Terminal() && tracker != nil {
				tracker.Poke() // it may have changed files
			}
		}
		if err := report(changed); err != nil {
			return err
		}
	}
	err = job.Err()
	stopped := errors.Is(err, context.Canceled)
	if err := report(tr.Finish(err, stopped, time.Now().UTC())); err != nil {
		return err
	}
	if stopped {
		return ctx.Err()
	}
	if failure != nil {
		return failure
	}
	if compact && err == nil && !compacted {
		fmt.Fprintln(diagnostics, "nothing to compact: the agent has not answered since the last compaction")
	}
	return err
}

// compactCommand recognizes "/compact [focus]" and returns the focus.
func compactCommand(prompt string) (string, bool) {
	rest, ok := strings.CutPrefix(strings.TrimSpace(prompt), "/compact")
	if !ok || rest != "" && !unicode.IsSpace(rune(rest[0])) {
		return "", false
	}
	return strings.TrimSpace(rest), true
}
