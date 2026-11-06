# Session compaction checkpoints

## Resume and history

After a successful model compaction, the coordinator appends a `checkpoint` record to the existing version-2 JSON Lines log. The checkpoint contains:

- The store head: sequence, session, exact turn/call/operation identity registries, external input IDs and pending-operation bookkeeping.
- The coordinator's replay state, including unresolved calls and their operation state.
- The context builder's summary, unanswered/staged inputs, relevant call descriptions, touched-file and notes metadata, and the budgeted earlier user prompts.

Settled operation bodies and idempotency data remain in their original records. A checkpoint keeps their identity/type/version/status and the original record's byte offset. An ID-only status after the checkpoint loads an archived body only when needed. Unfinished operations and terminal operations still awaiting a recorded outcome retain their full state.

The normal mapped resume path searches backwards for the latest valid checkpoint, restores its metadata and consumer state, and applies complete later records. The coordinator uses this short replay directly; it does **not** page archival `Items` from zero. A missing, unsupported or invalid consumer snapshot falls back to full item replay without partially changing the builder or loop.

`Items`, `Fork`, cockpit transcripts, metrics and `TranscriptSearch` continue reading the original history. A fast cached resume is not a complete `Items` cache. Forks encode their inherited history as before and do not inherit the parent's checkpoint or operation ownership. Rewind retains only checkpoints inside the unchanged prefix; checkpoints after the removed prompt disappear. Late operation records carried across a rewind are replayed after the retained checkpoint at their new offsets.

## Consistency and memory bounds

A checkpoint records its own starting byte offset and a SHA-256 digest of its data. It is usable only with the matching session header and supported checkpoint version. The newline is the commit boundary, as for other session records; this does not add a stronger fsync guarantee than existing appends.

An incomplete final checkpoint or later record is ignored and truncated by the next successful append. A syntactically valid hint with an invalid digest/version/offset is ignored in favor of an earlier checkpoint or ordinary replay. A complete malformed JSON record is corruption and still errors: acceleration does not make arbitrary committed corruption recoverable. Historical bytes are append-only; the checkpoint digest protects the snapshot, not a second checksum of the entire historical prefix.

Live progress refreshes are not recorded session items. The builder therefore keeps a shallow, short-lived recorded projection of its staged items when a progress refresh changes their contents/order. Checkpoints export that projection, not the transient progress display. Removed reasoning and transient placeholder counts are accounted for so resumed compaction reports agree with ordinary history replay. Failure IDs from committed tool results are remembered before masking/pruning can remove their `Error:` prefix; this keeps touched-file metadata consistent across later compactions without retaining the old error output. Only bounded earlier user prompts are exported (the existing 40,000-byte budget), with omission counts retained across repeated compactions. Current model/tools/system prompt and memory-file configuration are provided by the new run, not restored from an old checkpoint.

Exact turn/call/operation identity metadata and external input IDs **still grow with record count**. This is not an O(post-compaction-records)-only algorithm, nor a fixed-size cache. It avoids retaining/redecoding historical output bytes and does not duplicate old tool outputs in every checkpoint. Active unresolved work and its payloads are not artificially bounded. Runner/web stores use `MapLogs`; a store without mapping still reads the file before decoding, so its transient file-read allocation is not eliminated by this change. GOGC workarounds are unchanged.

## Compatibility and rollback

Existing version-2 logs without checkpoints remain readable. Older pre-version-2 formats remain unsupported exactly as before. An older binary that does not recognize the new `checkpoint` record kind cannot resume a checkpoint-bearing log; compatibility is not symmetric.

Do not let an older binary write the original checkpoint-bearing file. For a downgrade, stop its writer, make a separate backup/copy, and remove **only entire `checkpoint` JSON Lines records** from the copy. Keep the session header and every `item` and `operation` record, in order. Open the copy with the older binary. Checkpoints never replace archival data, so removing these hints does not remove user/model/tool history. No automatic downgrade or rewrite of existing user sessions is performed.

## Regression coverage and measurement

Tests cover legacy version-2 logs, multiple checkpoints and post-checkpoint records, pending operations and archived ID references, bad/unknown/partial checkpoints, torn and malformed suffixes, append recovery, mapped aliases, forks/ownership and historical identity guards, raw transcript/search/metrics readers, rewind across multiple checkpoints and moved operation records, malformed consumer fallback, removed tool translators, late input, unrecorded operation outcomes, transient progress ordering, future compactions and pruned-reasoning report counts.

`BenchmarkCheckpointResume` measures fresh mapped `Store.Resume`, coordinator restore and the first builder request. Its comparison forces the archival coordinator replay path while retaining the same fixture/checkpoint records. The default fixture is about 8 MiB. `KOU_CHECKPOINT_BENCH_TURNS` and `KOU_CHECKPOINT_BENCH_OUTPUT` select larger fixtures; generation is capped at 4096 turns and 512 MiB of estimated temporary log data. Every fixture belongs to the test's temporary directory. Retained heap is measured after GC while the restored coordinator, its store/cache and the first request remain live; mapped virtual/file-backed bytes are not heap bytes.

An initial 128-turn, 64-KiB-output sample produced an 8,629,063-byte log and a 43,998-byte latest checkpoint on Apple M4 Max:

| Path | Resume + first request | Allocated | Retained heap |
| --- | ---: | ---: | ---: |
| Forced full replay | 18.90 ms | 11,286,408 B | 401,800 B |
| Checkpoint | 0.775 ms | 392,592 B | 122,448 B |

These are single-sample fixture measurements, not a production latency promise.

A larger 256-turn, 512-KiB-output fixture used a 134,697,956-byte log and an 87,196-byte latest checkpoint. Three independent one-iteration samples on the final tested source measured:

| Path | Time range | Allocated range | Retained-heap range |
| --- | ---: | ---: | ---: |
| Forced full replay | 164.74–176.27 ms | 139,879,160–139,905,720 B | 682,616–693,128 B |
| Checkpoint | 3.723–3.746 ms | 751,248–752,184 B | 215,504–216,528 B |

At the medians this fixture resumed about 46× faster with about 186× fewer allocated bytes and 3.2× less retained heap. The larger fixture had 257 turn IDs, 256 call/operation IDs and 256 external input IDs; the earlier sample had 129/128/128/128. Doubling those counts increased the checkpoint from 43,998 to 87,196 bytes, consistent with the explicitly retained linear identity metadata. Increasing output bytes does not copy those archived outputs into the checkpoint.

Command: `KOU_CHECKPOINT_BENCH_TURNS=256 KOU_CHECKPOINT_BENCH_OUTPUT=524288 go test ./harness/coordinator -run '^$' -bench '^BenchmarkCheckpointResume$' -benchtime=1x -count=3 -v`. Raw output is in `issue-3-checkpoint-benchmark.txt`; its MB/s column divides the original log size by elapsed time, not physical bytes read. Fixtures were freshly written and indexed before timing; the OS page cache was not flushed, so these are warm-cache-oriented results, not cold-disk startup measurements. Mapping effects and this synthetic workload limit generalization. GOGC workarounds were not changed.

## Final verification

On the final source, `make test check` passed with per-command environment isolation:

```sh
env -u KOU_CANVAS_TOKEN -u KOU_CONVEYOR_TUNNEL_CONFIG \
  CC="$(xcrun --find clang)" SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX15.2.sdk \
  make test check
```

This includes `go test -race ./...`, the repository's non-race gateway tests and `go vet ./...`. Raw final output is in `issue-3-verification-final.txt`. Focused checkpoint tests were also rerun independently. The same-store `Resume → Items → Inspect → Fork → Resume` regression asserts that the coordinator performs zero archival `Items` reads after the cache upgrade; `Inspect` itself bypasses the shared cache.

Earlier attempts were not green: the inherited canvas environment activated an extra skill and failed `TestSkillsListsTheProjectsAndTheSystems` and `TestPluginListingHasTheSkills`; an isolated rerun encountered the unrelated exact-timing assertion in `TestRunMainHeartbeatReleasesWaitingBashAndReplays` (20 ms instead of 10 ms). That heartbeat test then passed ten race-enabled repetitions, and the final isolated full suite passed after the last source change. No unrelated test or environment configuration was changed.
