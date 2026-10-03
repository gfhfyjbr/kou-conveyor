# github-events

An example [plugin](../../../docs/plugins.md) that gives the browser
cockpit's canvas a source of events: **GitHub issues**, through the
[gh CLI](https://cli.github.com). Put a GitHub issues node on a canvas, name
its repository, and wire its outputs into an agent — a Claude Code
terminal, a kou agent — and each issue opened (or updated) in the
repository goes to it as a prompt:

```
Issue #42: Crash on save
https://github.com/owner/repo/issues/42

What happens, step by step…
```

- `plugin.json` declares the source in `canvas.sources`: what runs it
  (`python3 ./bin/github-issues`), every 60 seconds (`"mode": "poll"`),
  its settings (a JSON schema the canvas makes the node's form of), its
  outputs `opened` and `updated`, and the `template` an edge from it reads
  its events by unless the edge says another.
- `bin/github-issues` is the program. Each run reads its settings from the
  line of JSON on its standard input, asks `gh issue list` for the issues
  updated since the last run, prints a line of JSON for each, and keeps
  where it got to in `$KOU_CANVAS_STATE_DIR/cursor`. An event's `key`
  (`<number>:<updatedAt>`) lets the canvas drop an issue it was given
  already. Its first run starts from then on, not from the repository's
  history, unless **Backfill** is set; a `gh` that is not signed in says so
  on the node.

```sh
gh auth login   # once
cp -R path/to/examples/plugins/github-events ~/.config/kou-conveyor/plugins/
```

(on macOS, `~/Library/Application Support/kou-conveyor/plugins/`). The
canvas's **+ Add** menu lists **GitHub issues** among the sources at once.
