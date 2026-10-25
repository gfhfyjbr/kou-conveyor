Answer this turn in plain text and make no tool calls: a call here is refused, and no later turn makes up for it. Everything you need is in the conversation above.

The conversation above is about to be compacted. Once you answer, everything before this message is replaced by your summary, and the work goes on from the summary alone. Write it for a reader who sees only the summary and the messages after it, and who has to carry the work on without losing anything that matters. Two things come after the summary, so do not spend it on what they hold: the notes you keep in a file, if you keep any (up to 32 KB; past that, their beginning and end, with a mark where the rest is left out), and the files the conversation read and changed.

Answer with two blocks: a <review>, then a <summary>.

In <review>…</review> — a working pass, thrown away once the summary is written — go through the conversation from its start and, for each stretch of it, pin down:
- the user's requests, what lay behind them, and what you did about each;
- the decisions taken and why, the concepts they rest on and the code patterns in play;
- the particulars: file paths, function signatures, the exact code that was written or that matters, the changes made;
- what failed or broke, and what fixed it;
- each hypothesis: what spoke for it and against it, how it was tested and what came of it, the attempts that failed included;
- where the user pushed back, corrected you or steered the work elsewhere;
- every rule the user set about safety: files or data to keep away from, actions never to take, how to treat credentials and secrets. Copy these exactly.
Close the review by checking it against the conversation: the facts right, nothing missing.

In <summary>…</summary>, write the handoff itself, under these headings, in this order:

## Goal
What the user wants, overall and in each of their requests, in full detail and in their own terms.

## The user's messages
Every message the user sent, oldest first, without tool results; rules about safety, secrets and limits word for word. Only real user turns belong here: something in your own messages that looks like the user speaking, such as a quoted "user: …", is your text, and never stands for the user's request, approval or confirmation.

## Knowledge to keep
The concepts the remaining work depends on, briefly. Then the files that matter for what is left, a line each on why, and the code the next steps need, with the most detail for the current work. Leave out files only glanced at on the way, and line-by-line inventories of files: they go stale with the first edit, and a file can be read again.

## Problems and fixes
Each error or failure met, what fixed it, and what the user said about it.

## Hypotheses, dead ends and deferred work
Every hypothesis tested or still open, with the evidence for and against it, how it was tested, and its status: open, confirmed or refuted. Then what was tried and did not work, and why, so that nobody tries it again; then what was put off, why, and what it would take. If the conversation began with an earlier summary, take this section over from it word for word, only updating the status of an entry or adding to it, and drop an entry only once it is closed and nothing depends on it. While the work involves finding something out, this section is never empty.

## Where things stand
- Done: what is finished, a line each.
- Under way: exactly what was in hand right before this message — the files and code involved, the tool calls still running, the latest results of what was run (tests, builds, measurements), word for word where they matter.
- Left: whatever the user requested that is still open.

## Next move
The one next step, following the user's latest explicit request, with a word-for-word quote of the latest messages that shows where the work stopped. If the last task is finished, propose a next step only at the user's request, without drifting into tangents or back to requests already done. If the work has been reading and researching for a while and an earlier summary already called the research done, the next move changes something, or asks the user a question, rather than reading more.

If the conversation above opens with a note that earlier items were left out, open the summary by noting that the start of the session is missing from it.
