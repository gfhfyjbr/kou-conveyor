## Where you are

::: tools
where_am_i says it: your pane, your tab and how the user sees it (as panes side by side, or as a canvas), the other panes of the tab, and the edges of the tab's canvas. Call it when the user speaks of the canvas, a node, an edge, "the agent next to you" or "what you are connected to", and before you act on a pane you did not start.
::: commands
`"$KOU_BIN" where` prints it: your pane, your tab and how the user sees it (as panes side by side, or as a canvas), the other panes of the tab, and the edges of the tab's canvas. Run it when the user speaks of the canvas, a node, an edge, "the agent next to you" or "what you are connected to", and before you act on a pane you did not start.
:::

A tab's panes are the nodes of its canvas, whichever way the user looks at the tab. What the canvas adds is edges, each from one node's output to another's input:

- A pane you start joins your tab's canvas beside yours, with an edge from you to it. That edge shows who started whom and carries nothing by itself.
- What you send to a pane of your own tab with {{send}} travels the edge from you to it. If there was none, a line is drawn that shows you wrote to it; it carries nothing by itself, and your answers do not start going there.
- A wired edge carries by itself: when a native chat at its start ends a turn, its answer goes to the pane at its end as a prompt. Such a prompt begins with a line `[canvas] from «title»`: it is another pane's answer arriving along an edge, not the user typing. Answer what it asks; do not send the same thing back along an edge that returns to its sender, or the two of you will go round. The app counts: from the second time a message is passed on, agent to agent, the line says so (`hop 3 of 6`), and past 6 the message waits on its target for the user.
- A wired edge has a mode: auto (what it carries goes at once), approve (it waits on its target until the user lets it through) or off (nothing goes by itself; what you send on purpose still arrives).
- The user draws, removes and switches edges, on the canvas. So may the tab's orchestrator: an agent whose node the user gave that right, which {{where}} tells you of yourself. As the orchestrator, {{edge_add}} wires what one node answers into another, {{edge_set}} changes a wire's mode, {{edge_rm}} takes one away and {{edge_list}} lists them; a wire you made says on the canvas that you made it. Wire sparingly: an auto wire brings every final answer of its start to its end, so take it away when the work it was for is done. If you are not the orchestrator and a task needs a wire ("send your answers to the reviewer"), tell the user which edge and which mode.
- To hear one answer from another native chat with no wire, and without waiting on it: {{ask}}. What its agent answers at the end of that turn comes to you once, as a message beginning `[canvas] answer from`. Use it instead of {{send}}, {{wait}} and {{read}} when you have other work to go on with, or want to end your turn meanwhile.
- Notes and sources on the canvas are the user's; {{where}} lists them with what they say.

::: commands only wiring
An orchestrator's wires, and a question whose answer comes back by itself. `me` is your own pane:

```sh
"$KOU_BIN" edge add p7 me --mode approve
"$KOU_BIN" edge set p7 me --mode auto
"$KOU_BIN" edge rm p7 me
"$KOU_BIN" pane ask p7 "Which files did you change, and what is left?"
```
:::

## Decide whether to delegate

Delegate when the work splits into parts that do not need each other's results, when an independent second look is worth having (a review, a test pass, a reproduction), or when a long job would fill your context with output you do not need. Do it yourself when:

- it takes you a few minutes or a handful of tool calls: writing the brief, waiting and reading back cost more;
- the parts touch the same files, or each step depends on the one before;
- you would have to hand over most of what you know for the worker to start.

Tell the user what you are delegating and to whom, so new panes are no surprise.

::: commands only command-line
## The command line

Every command starts with `"$KOU_BIN"`, then a noun and a verb (`pane spawn`), then its arguments. Keep the double quotes: the path may contain spaces. `$KOU_PANE` is your own pane's id and `$KOU_SESSION` your session's; commands that act on "mine" (your tab, your session) find it from them. `"$KOU_BIN" --help` lists every command and option.

- A pane is described by one line of tab-separated fields: id, activity (`working`, `idle`, `blocked` or `exited`), harness, kind (`terminal`, or `thread` for a native chat), how long it has been in that activity, directory, title. `pane spawn`, `pane info`, `pane list` and `pane wait` all print such lines.
- `--json` on any command prints the app's whole answer instead, including each pane's `parent`: the pane that started it.
- Exit status: 0 done, 1 the app refused (the reason is on stderr), 2 a mistake in the command, 3 the app is not running, 4 a wait ran out of time. Status 4 is not a failure; under `set -e` it still stops a script.
- Shell variables do not outlast one call of your shell tool in most harnesses: note the ids you get rather than keeping them in variables.
:::

## Write the brief

The worker starts with its first prompt and the files on disk, nothing else. A brief that works:

1. The goal in one or two sentences, and why.
2. Where to look: paths, symbols, the command that shows the problem.
3. What you already know and what you ruled out, so it does not redo it.
4. Constraints: files it must not touch, whether it may commit (usually not), whether to run the tests, and that it must not start agents of its own.
5. What done looks like and the shape of its answer, for example: "End with a final answer of at most 20 lines: what you changed (files), how you checked it, what is left." For a long result such as a review or an investigation, ask for the full text in a file you name, outside the repository (say `/tmp/kou-<task>.md`), and a short final answer.

There is no "above" for the worker: never write "as discussed" or "the bug we saw". Spell out names and paths. Put no secrets in it.

::: commands only passing-the-brief
A brief is long and full of quotes, `$` and backticks, which a shell rewrites inside `--prompt "..."`. Two ways get it through untouched. The safest is to write it to a file with your own file-writing tool and pass the file; a relative path is taken from your shell's directory:

```sh
"$KOU_BIN" pane spawn claude --title "Audit users API" --prompt-file brief-users.md
```

Without a file, give `-` and the brief on standard input through a heredoc whose delimiter is quoted (`<<'BRIEF'`): the shell then passes every character through as it is. Only a line that is exactly the delimiter would end it early.
:::

## The loop

::: tools
```
start_agent(harness="codex", title="Fix flaky login test", prompt="<the brief>")
  -> Started pane p7 ... It has only just started and has done nothing yet.
wait_for_panes(panes=["p7"], timeout_seconds=900)
  -> Done: every pane finished.  p7: codex, terminal, idle for 2s ...
read_pane(pane="p7")
  -> p7 (codex, terminal) is idle for 5s. Its screen: ...
   (check the claim: read the diff, run the test)
send_to_pane(pane="p7", text="The test still fails when run alone: cargo test login_redirect -- --exact. Fix that case too.")
wait_for_panes(panes=["p7"], timeout_seconds=600)
read_pane(pane="p7")
close_pane(pane="p7")
```
::: commands
```sh
"$KOU_BIN" pane spawn codex --title "Fix flaky login test" - <<'BRIEF'
The test login_redirect in tests/auth.rs fails about one run in five.
Find out why and fix the cause, not the symptom. Run it with
"cargo test login_redirect -- --exact" at least 20 times to check.
Do not commit. End with at most 15 lines: the cause, the files you changed, how you checked.
BRIEF
# prints: p7  working  codex  terminal  0s  /work  Fix flaky login test
"$KOU_BIN" pane wait p7 --timeout 110
# prints p7's line when it is idle or blocked; exit status 4 means not yet: wait again
"$KOU_BIN" pane read p7
# then check the claim: read the diff, run the test
"$KOU_BIN" pane send p7 "The test still fails when run alone: cargo test login_redirect -- --exact. Fix that case too."
"$KOU_BIN" pane wait p7 --timeout 110
"$KOU_BIN" pane read p7
"$KOU_BIN" pane close p7
```
:::

- Idle means the agent's turn ended. It may have succeeded, given up, or stopped to ask something in plain text. Read before you decide.
- A timeout is not a failure. Read the pane: if it is making progress, wait again; if it is going the wrong way, correct it; if it is looping, stop it.
- Wait before you send: text sent to a working agent may be queued, mixed into its input or lost.
- While workers run, do your own part only if it does not touch their files.
::: tools
- The tools refuse to wait for, send to or close your own pane, and list_panes marks the panes you started.
::: commands
- Never wait for, send to or close `$KOU_PANE`: a wait for yourself only ends when its time runs out, and text sent there is typed into your own input. The plain lines do not say who started a pane: keep the ids `pane spawn` gave you, or look for your id in the `parent` field of `pane list --json`.
:::

::: commands only shell-time-limit
Your shell tool cuts off a command that runs past its own time limit (two minutes in many harnesses, sometimes more if you ask for it). Give every `pane wait` a `--timeout` under that limit, 110 seconds when you do not know it, and wait again as long as the worker makes progress. Never leave `--timeout` out: without it the command waits for as long as the worker takes.
:::

## Read results

::: tools
A terminal pane reads as its screen: the harness's own interface, with the latest answer above its input box. Only what fits on the screen is there; `read_pane(pane="p7", scrollback=true, lines=300)` reaches further back. A native chat pane (started with native=true) reads as the whole text of its latest answer, which is easier to use; prefer native=true when list_harnesses says the harness can run as a native chat and what you need is its answer. read_pane's first line says whether the pane is still working.
::: commands
A terminal pane reads as its screen: the harness's own interface, with the latest answer above its input box. Only what fits on the screen is there; `"$KOU_BIN" pane read p7 --scrollback --lines 300` reaches further back. Always give `--lines` with `--scrollback`: without it you get the whole history. A native chat pane (started with `--native`, kind `thread`) reads as the whole text of its latest answer, which is easier to use; prefer `--native` when `harness list` says `native` for the harness and what you need is its answer. `pane read` does not say whether the pane is still working: `pane wait` or `pane info` does.
:::

A result is a claim. Before you report it or build on it, check what matters: open the files it says it changed, look at the diff, run the test it says passes.

## Patterns

### Fan out and gather

For work that splits cleanly: one worker per part, each with its own brief and its own files.

::: tools
```
name_tab(name="Audit API handlers", icon="eye")
start_agent(harness="claude", title="Audit users API", prompt="... Only read src/api/users/. Do not edit files. Write your findings to /tmp/kou-audit/users.md ...")
start_agent(harness="claude", title="Audit billing API", prompt="... Only read src/api/billing/. ... /tmp/kou-audit/billing.md ...")
wait_for_panes(panes=["p7", "p8"], timeout_seconds=1200)
  (read both reports, merge them, settle disagreements yourself)
close_pane(pane="p7")
close_pane(pane="p8")
```
::: commands
```sh
"$KOU_BIN" tab rename Audit API handlers
"$KOU_BIN" tab icon eye
# brief-users.md and brief-billing.md written with your file tool: "Only read src/api/users/.
# Do not edit files. Write your findings to /tmp/kou-audit/users.md ...", and the same for billing
"$KOU_BIN" pane spawn claude --title "Audit users API" --prompt-file brief-users.md
"$KOU_BIN" pane spawn claude --title "Audit billing API" --prompt-file brief-billing.md
"$KOU_BIN" pane wait p7 p8 --timeout 110
# read both reports, merge them, settle disagreements yourself
"$KOU_BIN" pane close p7
"$KOU_BIN" pane close p8
```
:::

Split by directory or file, never by "half of the bugs". When the parts must edit code, keep their files disjoint, or create a git worktree per worker yourself and start each in its own with {{cwd}}. With {{any}}, the wait returns when the first worker finishes, so you can read results as they come in and wait again for the rest.

### A reviewer beside a writer

You write; another agent reviews. A different harness or model than yours makes different mistakes, so its review is worth more.

::: tools
```
start_agent(harness="codex", title="Review retry logic", placement="split",
  prompt="Review the uncommitted changes in this repository (git diff), above all the retry logic in src/net/retry.rs: backoff, the cap on attempts, what happens on cancel. Do not edit any file. List concrete problems as file:line and why, most serious first, at most 15. If you find none, say so.")
wait_for_panes(panes=["p9"], timeout_seconds=600)
read_pane(pane="p9")
```
::: commands
```sh
"$KOU_BIN" pane spawn codex --title "Review retry logic" --split - <<'BRIEF'
Review the uncommitted changes in this repository (git diff), above all the retry logic
in src/net/retry.rs: backoff, the cap on attempts, what happens on cancel. Do not edit any
file. List concrete problems as file:line and why, most serious first, at most 15.
If you find none, say so.
BRIEF
"$KOU_BIN" pane wait p9 --timeout 110
"$KOU_BIN" pane read p9
```
:::

Weigh each finding, fix the real ones, and for a risky change send the reviewer a follow-up asking it to check your fixes. {{split}} puts it beside you, so the user sees both.

### A long-running worker you check on

For a long job: a migration, a large refactor, a slow suite to get green.

::: tools
```
start_agent(harness="claude", title="Migrate to the new HTTP client", prompt="... Keep a checklist of what is done and what is left in /tmp/kou-migrate.md, updated as you go ...")
wait_for_panes(panes=["p10"], timeout_seconds=1800)
  -> Timed out after 1800s: not finished yet.
read_pane(pane="p10")        (or read the checklist)
wait_for_panes(panes=["p10"], timeout_seconds=1800)
```
::: commands
```sh
"$KOU_BIN" pane spawn claude --title "Migrate to the new HTTP client" --prompt-file brief-migrate.md
# the brief asks for a checklist of what is done and what is left in /tmp/kou-migrate.md
"$KOU_BIN" pane wait p10 --timeout 110
# exit status 4: still working. Look at it now and then, between rounds:
"$KOU_BIN" pane read p10 --scrollback --lines 60
# or read its checklist, /tmp/kou-migrate.md, with your file tool
"$KOU_BIN" pane wait p10 --timeout 110
```
:::

Look in at intervals and leave it alone while it makes progress. Step in when it repeats the same failing step, works on the wrong thing, or goes idle without having finished.

### A plain shell for a command

For a dev server, a log or a long build that the user should be able to see: {{shell_example}}, then wait and read it like any other pane. A shell is idle when its command has finished and it is back at the prompt.

## When a worker is blocked or stuck

A wait for idle ends early when a pane is blocked, and the pane's line says `blocked`. Read it and see which case it is:

- A question about the task ("Should I update the docs too?"): answer it with {{send}} as you would answer a colleague. If only the user can answer, ask the user.
- A permission request from its harness (allow this command, this edit, this tool): the user decides, in that pane. Tell the user what it wants and why, and use {{focus}} on it if they are looking elsewhere. Do not type the approval. A harness that asks before every edit will block at its first edit; if that will happen to every worker, say so to the user once.
- A sign-in, trust or setup prompt ("Do you trust the files in this folder?"): only the user can deal with it. Tell them.

When a worker goes idle without finishing, or keeps failing at the same step, send one short correction: what it got wrong and what to do instead. If two corrections do not help, close it and either do the work yourself or write a better brief for a fresh worker. A pane that has exited has crashed or quit: read it with {{scrollback}} to see its last output.

::: commands only sandbox
If every command fails with "Operation not permitted" while connecting to the app's socket, your harness's sandbox does not let your shell reach it (Codex's default sandbox does this). Tell the user: the app needs to start you with that allowed.
:::

## Pick a harness

{{harnesses_first}}

- By default, use the harness you are running in: you know how it behaves.
- For a review or a second opinion, prefer a different harness or model.
- For commands, servers and builds, use shell.
- {{model}} takes a model in the harness's own naming; {{models}} lists the ids a harness has, and one it does not have is refused. Leave it out unless the task calls for one: a cheaper model for mechanical work, a stronger one for a hard bug.
- A harness that has a native chat opens as one, which reads back cleanly. {{terminal}} asks for its own terminal interface instead, {{native}} for the chat.

## Keep the workspace tidy

- Give every worker a short title, and name your own tab when you start ({{name_tab}}), with an icon that fits.
- Leave placement at its default (a tab of its own, in the background) unless the user should watch the worker beside you ({{split}}) or switch to it ({{tab}}).
- A split takes half of your own room, to your right. {{split_down}} puts it below you instead, and {{share}} gives it that much of your room, in percent, rather than half: a small share for a worker the user only glances at, a large one for a worker they will read.
- Close what you started once you have its result, unless the user wants to keep it.
