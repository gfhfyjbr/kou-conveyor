package main

import (
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"flag"
	"fmt"
	"io"
	"os"
	"path/filepath"
	"regexp"
	"strings"
	"time"

	"github.com/gfhfyjbr/kou-conveyor/cmd/internal/sessionmetrics"
)

// kou-conveyor-web metrics: what sessions cost and where it went, from
// their logs.

const metricsUsage = `Usage: kou-conveyor-web metrics [options] SESSION...

Measures sessions from their logs: what their requests cost, how much of it
went to writing the provider's cache anew and why, the requests that only
waited, the messages, compactions, heartbeats and stops. Costs are in units
of an uncached input token: a token read from the cache 0.1, one written to
it 2 (1.25 with -ttl 5m), an output token 5.

SESSION is a session log (.harness/sessions/ID.session.jsonl), or the ID of
a session of the workspace the command runs in, or the start of one.

Options:
`

func metricsMain(args []string, stdout, stderr io.Writer) int {
	f := flag.NewFlagSet("kou-conveyor-web metrics", flag.ContinueOnError)
	f.SetOutput(stderr)
	f.Usage = func() {
		fmt.Fprint(stderr, metricsUsage)
		f.PrintDefaults()
	}
	since := f.String("since", "", "count from this time on: `TIME` in UTC as in the logs (2026-10-08T22:16), with an offset (2026-10-09T01:16+03:00), or a duration back from now (2h)")
	ttl := f.Duration("ttl", time.Hour, "how long the provider keeps the cache: a longer pause expires it; 5m or less makes a cache write cost 1.25 instead of 2")
	wait := f.String("wait", sessionmetrics.DefaultWait.String(), "the shell commands that only wait, a `REGEXP`; the canvas's waits count anyway")
	asJSON := f.Bool("json", false, "print JSON instead of markdown")
	list := f.Bool("list", false, "list the requests that rewrote the cache or started cold")
	if err := f.Parse(args); err != nil {
		if errors.Is(err, flag.ErrHelp) {
			return 0
		}
		return 2
	}
	if f.NArg() == 0 {
		f.Usage()
		return 2
	}
	options := sessionmetrics.Options{TTL: *ttl}
	var err error
	if options.Since, err = parseSince(*since, time.Now()); err != nil {
		fmt.Fprintln(stderr, "kou-conveyor-web metrics: -since:", err)
		return 2
	}
	if options.Wait, err = regexp.Compile(*wait); err != nil {
		fmt.Fprintln(stderr, "kou-conveyor-web metrics: -wait:", err)
		return 2
	}
	type measured struct {
		File   string                `json:"file"`
		Report sessionmetrics.Report `json:"report"`
	}
	var all []measured
	failed := false
	for _, name := range f.Args() {
		path, err := sessionLog(name)
		if err == nil {
			var report sessionmetrics.Report
			if report, err = measure(path, options); err == nil {
				all = append(all, measured{File: path, Report: report})
				continue
			}
		}
		fmt.Fprintf(stderr, "kou-conveyor-web metrics: %s: %v\n", name, err)
		failed = true
	}
	if *asJSON {
		data, err := json.Marshal(all, jsontext.WithIndent("  "))
		if err != nil {
			fmt.Fprintln(stderr, "kou-conveyor-web metrics:", err)
			return 1
		}
		fmt.Fprintf(stdout, "%s\n", data)
	} else {
		for _, m := range all {
			title := strings.TrimSuffix(filepath.Base(m.File), ".session.jsonl")
			if err := m.Report.Markdown(stdout, title, *list); err != nil {
				fmt.Fprintln(stderr, "kou-conveyor-web metrics:", err)
				return 1
			}
		}
	}
	if failed {
		return 1
	}
	return 0
}

func measure(path string, options sessionmetrics.Options) (sessionmetrics.Report, error) {
	file, err := os.Open(path)
	if err != nil {
		return sessionmetrics.Report{}, err
	}
	defer file.Close()
	return sessionmetrics.Read(file, options)
}

// sessionLog finds the log of a session: name is its path, or the ID of a
// session of the workspace here, or the start of one.
func sessionLog(name string) (string, error) {
	if info, err := os.Stat(name); err == nil && !info.IsDir() {
		return name, nil
	}
	if strings.ContainsAny(name, `/\*?[`) || name == "" {
		return "", fmt.Errorf("no such session log")
	}
	dir := filepath.Join(".harness", "sessions")
	matches, err := filepath.Glob(filepath.Join(dir, name+"*.session.jsonl"))
	if err != nil {
		return "", err
	}
	switch len(matches) {
	case 0:
		return "", fmt.Errorf("no session log here or in %s", dir)
	case 1:
		return matches[0], nil
	}
	return "", fmt.Errorf("%d sessions in %s start so; give more of the ID", len(matches), dir)
}

// parseSince reads -since: a time in UTC, one with an offset, or a duration
// back from now.
func parseSince(value string, now time.Time) (time.Time, error) {
	value = strings.TrimSpace(value)
	if value == "" {
		return time.Time{}, nil
	}
	if d, err := time.ParseDuration(value); err == nil {
		return now.Add(-d).UTC(), nil
	}
	for _, layout := range []string{time.RFC3339Nano, "2006-01-02T15:04Z07:00"} {
		if t, err := time.Parse(layout, value); err == nil {
			return t.UTC(), nil
		}
	}
	for _, layout := range []string{"2006-01-02T15:04:05.999999999", "2006-01-02T15:04", "2006-01-02 15:04:05", "2006-01-02 15:04", "2006-01-02"} {
		if t, err := time.ParseInLocation(layout, value, time.UTC); err == nil {
			return t, nil
		}
	}
	return time.Time{}, fmt.Errorf("%q is neither a time (2026-10-08T22:16, in UTC) nor a duration (2h)", value)
}
