# Rust migration compatibility harness

The existing Go implementation is the oracle during the Rust migration. Rust
production binaries must not depend on it. The current compatibility slice uses
`harness/sessionstore/localfile/testdata` directly, without copying user data
into tracked fixtures.

Run native Rust checks from the repository root:

```sh
make rust-check rust-test
make rust-build       # optional release binaries, no installation
make rust-compat      # read and mixed-writer Go/Rust checks
```

Build output is in the already ignored `tmp/rust-rewrite-target/`. The isolated
`benchmarks/rust-go` experiment remains outside this workspace.

## First read-only vertical slice

```sh
python3 compat/inventory.py --check
python3 compat/store_parity.py
cargo run -p kou-session -- inspect path/to/ID.session.jsonl
cargo run -p kou-session -- items path/to/ID.session.jsonl 0 20
```

The Go oracle is development-only (`cmd/kou-conveyor-oracle`). Archive parity
checks all persisted items and resumed operations, not the consumer checkpoint
codec or accelerated Resume boundary. `resume-full` explicitly performs the
full replay fallback. The reader ignores an uncommitted final line and rejects
stale indexes after append, truncate or inode replacement. It does not mmap or
change the original log. The initial reader has a configurable 256 MiB record
limit; arbitrary deeper/large-input compatibility needs the complete fuzz gate.

Build/run the actual Rust/WASM client:

```sh
rustup target add wasm32-unknown-unknown
cargo install wasm-bindgen-cli --version 0.2.130 --locked --root tmp/rust-tools
python3 compat/build_web.py
cargo run -p kou-conveyor-web-rs -- --session-dir path/to/sessions --assets tmp/rust-web-assets
```

The server binds loopback only (127.0.0.1:8091 by default). Its new migration API
serves metadata pages, on-demand records capped at 1 MiB, and an untruncated
streaming JSONL export. It is not yet a replacement for the old cockpit API.
The browser keeps one 50-record page and one inline body; it displays durable
records, not the complete folded chat transcript. Large records are available
via export, not silently truncated. The LRU uses conservative allocation-size
estimates, not an allocator-enforced hard RSS bound.

HTTP/security tests are in `crates/kou-server/tests/http.rs`. Browser smoke:

```sh
cargo build -p kou-conveyor-web-rs
npm install --prefix tmp/rust-browser playwright
node compat/browser_smoke.mjs
```

The browser test uses a fresh headless Chrome profile, copies public fixtures
into its own temporary directory, starts/stops only its own Rust server, and
writes desktop/mobile screenshots to the repository root.

## Writer compatibility

`python3 compat/writer_parity.py` creates fixtures in private temporary directories.
It checks Rust create/append/fork with Go replay, Go append with Rust replay,
checkpoint envelopes and recovery tails in both directions, continuation of a Go
fixture, and exclusion/recovery with the actual Go `cockpit.LockSession`.
The native library writer uses atomic publication, per-record fsync, the shared
`.locks/ID.lock` protocol, and rejects a changed source. It is not exposed as an
HTTP write API. The failure-injection test also checks that an I/O failure poisons
the writer rather than retrying an uncertain commit.

`kou-session resume FILE` uses the latest valid version-1 checkpoint envelope,
checks its SHA-256/offset/head, and replays only the suffix. Invalid/newer/torn
checkpoints fall back to an earlier valid checkpoint or full replay. Forks in
the suffix force full replay so inherited operations never become dispatchable.
Archived operation bodies are loaded only when a saved-ID reference needs them.
Consumer state is preserved as opaque JSON; its application-level codecs remain
a separate runner migration gate. `resume-full` is the independent full-replay
reference path. Real input logs are copied to mode-0600 private snapshots for
read comparisons; a source mutation during copying fails the check.

## Recovery resource measurements

On macOS, `python3 compat/recovery_bench.py FILE...` builds release readers and
runs interleaved fresh processes over mode-0600 private copies (five repetitions,
warm file cache). Reports go to the workspace root. It measures the development
Go oracle versus the Rust session CLI, including startup and JSON serialization;
linking/startup are not normalized. This is not an E2E runner/server/browser
benchmark. Keep the unoptimized report and any optimized report separately so
memory gains do not hide CPU regressions. `/usr/bin/sample` can profile the Rust
CLI without reading or logging session bodies. Optional
`--synthetic-checkpoint-mib 64` creates a private giant-record fixture with a
small opaque checkpoint. This is not a runner-state benchmark: compare fast
recovery with full fallback and inspect its peak memory too (owned giant raw
records currently cause a large fallback spike).

## Code VM candidate

`kou-runtime` hosts QuickJS with explicit host capabilities, promises, pending-call
and call-count limits, a total pending-argument byte budget, parent cancellation,
CPU interruption, memory/stack limits
and bounded Unicode console/result/call output. `CodeHost` must enqueue without
blocking, bound its native queues, honor `wait` deadlines and cancel all work.
JS handles stay on one VM thread; only owned JSON messages may cross workers.
QuickJS is a C engine with its own GC; this is not a hard OS sandbox or an all-safe
Rust claim. The runtime has no Go/goja dependency. `NativeHost` now supplies
Read/Write/Edit/ApplyPatch, ReadText/Skill, Bash and external command plugins through
a per-execution worker pool. Reporting, image handles, transcript search, full
option/error edge cases and runner integration remain open.

```sh
cargo test --locked -p kou-runtime
python3 compat/codevm_parity.py
```

The differential corpus uses only fake plugin tools and rejected built-in
arguments. The Go oracle still has the original native capabilities: never feed
it untrusted or user-supplied code. Only engine stack frames and timestamps are
excluded from comparisons, not call order or output bodies. The projection maps
Go's PascalCase Result fields onto the experimental Rust Execution DTO; production
Result wire casing/timestamps remain a separate integration gate.

## Native tools and process host

```sh
cargo test --locked -p kou-tools -p kou-runtime
python3 compat/fileops_parity.py
python3 compat/patch_parity.py
python3 compat/native_parity.py
```

The first two compare 183 Read/Write/Edit and 122 ApplyPatch fixtures, including
exact final file bytes/modes/symlinks. The native VM corpus runs 29 trusted scripts
against real temporary files, shell processes and command plugins; it compares
values/logs/call outcomes plus file state. All writes stay in disposable fixture
trees. Do not feed arbitrary user code to either Go VM oracle. The file-tool oracle
uses a json/v1 rune-replacement display projection for invalid UTF-8, while byte
preservation is checked separately; this is not a json/v2 wire-parity claim.

Reads retain only the displayed prefix even on giant physical lines and check
cancellation between input chunks. Edits preserve untouched non-UTF-8 bytes and
follow live symlinks. Alias-aware sorted locks serialize Edit/Write/Patch, including
new files under a symlinked directory whose descendants do not exist yet. Patch
validation stages all changes before publication, but an I/O failure during a
multi-file publication is **not** an atomic rollback, as in the Go implementation.

Intentional resource policies need release compatibility review: Edit accepts at
most 32 MiB of input and 64 MiB of expanded output; ApplyPatch accepts 4 MiB of
patch text and 64 MiB of total staged content. ReadText has a 4 MiB cap (the Go Code
helper was unbounded). The host defaults to four workers, 64 admitted jobs and a
16 MiB aggregate argument budget. Completed results have a count bound; there is
no claim that these are a hard whole-process RSS bound. Patch line metadata,
transient JSON/edit buffers and filesystem blocking remain resource gates.

Processes have a separate group and nonblocking stdin/stdout/stderr on one worker,
with head/tail-bounded captures and no detached copying threads. Cancellation and
normal leader exit terminate descendants with TERM then KILL after the grace
period, reap the leader and close all owned pipes. Host cancellation joins its
workers even if an external Rc retains the host. Tests cover a TERM-ignoring loop,
blocked stdin, live descendants, bounded output, explicit empty environment and
CLOEXEC file identity (not just whether a shell reused the descriptor number).
This is not an OS sandbox: a child which escapes into another session is outside
the process-group boundary.

`run_captured_process` instead directs the child to existing regular capture files,
not pipes. Both descriptors are validated, including inode aliases, before either
file is truncated. It keeps no output body in RAM, syncs captures after owned
process completion and exposes the raw exit code and signal separately. Start
callback errors/panics kill and reap the invocation. Eight native tests include
invalid/symlink/FIFO/aliased paths, callback cleanup, blocked stdin, timeout/cancel,
descendants and CLOEXEC identity. **Spool disk is not capped.**

`python3 compat/capture_parity.py` adds **19 owned Go/Rust comparisons** of exact
output bytes, modes, raw exit/signal, stdin/environment/cwd, normal descendant
cleanup and cancellation-time final output. Each oracle has private temporary
artifacts and runs only trusted fixtures; the projection is not full primitive
control/event or diagnostic conformance. Persistent process status/attachment/
control and the full primitives operation service remain open.

## Responses provider codec, SSE, HTTP and client wrappers

```sh
cargo test --locked -p kou-provider
python3 compat/provider_parity.py
```

This runs 433 comparisons, including 161 actual Go/Rust loopback HTTP exchanges
and 89 synthetic Codex credential/config cases.
It compares request JSON, selected auth/cache headers, final DTOs/errors/API
metadata, rate-limit waits, overflow sizes, delta/reset events, warnings, retries
and traces. Both sides only contact their own fake server; the five provider
clients use fixed synthetic credentials, never production ones or paid requests.
Deterministic delay hints allow exact retry comparisons; jitter ranges are unit
tested, not normalized away. The Rust example is named `responses_oracle` to
avoid a shared Cargo output collision with the Code VM `oracle` example.

The request wire view borrows history text/arguments/schemas instead of cloning
an owned `serde::Value` conversation tree. Tests cover cross-model replay,
reasoning/raw metadata, missing/late tool results, images, argument repair,
function/hosted tools, cache keys and extension collisions. Response decoding
preserves opaque usage/reasoning values and uses borrowed text unless JSON
escapes need decoding.

SSE framing handles LF/CRLF/CR and arbitrary chunk cuts; unfinished frames at EOF
are not emitted. The streaming state buffers output-item objects by sparse index,
uses observed items only when terminal output is absent/empty, and preserves the
other terminal fields. Text/reasoning summary deltas and paragraph separators
match the Go reporter. Tests exercise EOF, in-band errors, authoritative terminal
output, duplicates, null/scalar items and single-byte chunks.

The Rust HTTP adapter implements cancellation, optional absolute deadlines,
30-minute network-idle deadlines that exclude reporter processing, retries and
upstream delay hints, reset parsing, typed rate-limit/overflow errors and image
fallback. OpenAI, Fireworks, OpenRouter, Ollama and Codex wrappers implement their
actual cache/auth headers and usage semantics. Native tests cover headers/body/backoff/
admission cancellation, dropped futures, early terminal return, slow callbacks,
gzip and byte limits. No production credential files are read by these tests.

Configurable defaults are 32 MiB encoded request, 1 MiB HTTP error, 16 MiB SSE
frame, 32 MiB retained payload, 4096 items, eight admitted requests and 256/64 KiB
text-only-model cache entries/bytes. These are release compatibility policies,
not a hard global RSS bound. The Go SSE frame ceiling was 256 MiB. Rust's
30-second connect deadline includes TLS; Go's separate 10-second TLS handshake
deadline is not replicated. Live TLS/proxy compatibility and provider resource
measurements remain open. Codex has synthetic credential/config parity, private
constructor-snapshot files, literal-endpoint/redirect/header checks and no automatic
login/refresh. Its explicit-token cap is 1 MiB, a new release policy. Typed provider
objects reject positional arrays without cloning into a JSON value tree. Wider
domain DTO shape fuzzing and runner credential/configuration integration remain
open. See `rust-rewrite-provider-2026-11-09.md`.

## Anthropic Messages

```sh
python3 compat/anthropic_parity.py
```

This adds 355 comparisons (222 codec, 56 SSE, 77 owned-loopback HTTP including
model listings). The public Rust adapter implements borrowed requests, model and
thinking policies, cache breakpoints, streaming prefix hashes, signed reasoning,
exact streamed tool arguments, usage, retries and thinking/context/fallback
recovery, context room/overflow, and model pagination. Comparisons include full
endpoint queries, selected header values, prefix reports, errors and callbacks.
Image validation uses fixed scratch rather than allocating a full decoded image.

Anthropic framing deliberately differs from Responses: bare CR is not a line
separator, no BOM is stripped, and message_stop is read through to EOF, so a later
API error still wins. Its overload delay is linear with no 60-second cap and
ignores Retry-After; it must not share the Responses overload policy. Native tests
cover byte/depth caps, all stream chunk cuts, cancellation/deadline/drop/admission,
ping activity, model pagination and page-cycle caps. The Go Anthropic package and
race-enabled HTTP/recovery/pagination oracle fixtures also passed race checks.

Anthropic defaults to a 10-minute per-await idle timeout, 100 model pages,
100,000 model names and 8 MiB of aggregate name bytes. Request/error/frame/retention
and admission limits otherwise match the table above; the schema sort depth is
128. Go's SDK line ceiling is 32 MiB, not the Responses ceiling. These caps and
refusing redirects by default are new release policies. Slow reporter processing
is excluded from Rust's next network-idle interval; Go's watchdog can count that
time. SDK malformed/duplicate/rare union shapes and real TLS/proxy/resource
measurements remain release gates. No paid provider calls were made.

## Context, compaction and consumer checkpoints

```sh
python3 compat/context_parity.py
```

`kou-engine::context::Builder` implements the actual pure committed/staged context
state, running/progress results, settings and heartbeat messages, linked files,
pruning/masking/thinking/image windows, provider usage estimates, compaction
request fitting and summary extraction, prior user messages, notes/touched files,
and context checkpoint version 1. Both provider adapters accept its shared
history directly. Request snapshots share `Arc<LlmItem>` bodies; changed items are
replaced rather than mutating an in-flight snapshot. The extra answered-prompt
projection is bounded to 40 KB before compaction, not just when checkpointed.

The corpus has **285 scenarios**, including 80 seeded histories and 82 valid,
truncated or corrupted PNG/GIF/JPEG/WebP header probes. It compares exact model
requests, reports, estimates, compacted text, notes and checkpoint continuations
in both directions. Fourteen native tests cover ownership, durable versus live
progress, prompt budgets, restoration, pruning counts and file failures, notes,
images and usage. Go contextbuilder race tests and context-oracle vet passed.

A dimensions-only image library was rejected after 27/29 controlled corrupted
header samples disagreed with Go. The replacement streams the registered Go
header/config policies and CRC checks, without buffering a decoded image or
pixel data. Broader image and typed-field/array/checkpoint diagnostic fuzzing
remain release gates. Context working bodies and in-flight snapshots are not a
hard RSS budget; no CPU/RSS advantage for this slice has been measured.

## Coordinator replay and recovery

```sh
python3 compat/replay_parity.py
```

`kou-engine::coordinator::History` restores recorded inputs, turns, model
responses, tool results, controls and forks without dispatching effects. It saves
and restores coordinator checkpoint version 1, shares active call bodies with
context, and uses the real cold session store for accelerated recovery or full
fallback when the consumer checkpoint is invalid. Recovery checks source
freshness before and after reading, including a checkpoint-only restore.

The oracle invokes the actual Go coordinator, stopping at Context.Resumed before
scheduler/model/tool execution. **138 replay scenarios** (60 seeded histories)
and **19 Rust-exported coordinator checkpoint import/fallback matrices** pass.
The development oracle now probes scalar state and operation headers as well as
built context: context-only comparisons had missed pending-input, compaction,
verification and stop bookkeeping differences. Those mismatches were corrected
from the current Go loop. A body-free `History::inspect` projection never builds
or prunes context. This is not independently exported bidirectional coordinator
checkpoint parity. Eleven native replay tests cover ordering, missing operations,
transactional restore, shared ownership, real-store fast/full recovery, freshness,
cross-session rejection and restored verification markers.

## Live coordinator core

`coordinator::Coordinator` is a single-owner Tokio loop around the same reducer.
It schedules pure tool translations, commits response/status records before
operation dispatch, persists operation updates before delivering results, and
handles steering, hard/when-idle stop, cancellation, grace, after-tools input,
verification, heartbeat/report timers, run budgets, quota backoff and overflow
compaction retries. It owns one model future; steering cancels and drops that
future rather than retaining a stale response task.

`FileJournal` moves the real `SessionWriter` onto one bounded, dedicated writer
thread. Drop closes the queue and joins the writer, including when a waiting
future is dropped. Recovery dispatches the recorded operation with its original
ID, not a newly translated call. This does not prove exactly-once external effects.
Concrete provider adapters implement the model port without Go. Native tests drive
an actual HTTP/SSE adapter against an owned loopback server and exercise a real
private FileJournal restart through cold-store recovery.

**43 native driver tests** cover these boundaries and policies. A real FileJournal
matrix checks that empty/review-only summaries do not write a consumer checkpoint,
that successful summaries do, and that cold recovery preserves the right context.
Durable errors retain Go's input/turn/response/call/operation context.

```sh
python3 compat/loop_parity.py
```

The independent live corpus runs the actual Go and Rust coordinators through
**44 scripted scenarios**. Both use owned temporary real session journals, inert
operation managers and scripted model replies: no provider or tool effects run.
It compares ordered commits, complete model requests, dispatches, updates,
cancellations, checkpoint presence and errors. Only generated identity fields
and their quoted diagnostic references are remapped; opaque payloads and provider
values are left untouched. Checkpoint bodies are not compared by this suite.

Coverage includes steering/stale replies, ordered multi-operation completion,
deferred-input ordering, hard/idle stop, duplicate input, in-flight settings and
heartbeat controls, compaction, verification, quota retries, run budgets and
injected durable failures. Barriers synchronize observable actions, not sleeps.
The corpus exposed operation-order, deferred-prompt-order and non-steering-control
bugs missed by native tests; regression tests now cover those fixes. A successful
initial status without an operation is rejected by both real stores, rather than
accepted by an in-memory journal. This is a bounded corpus, not exhaustive live
scheduler, crash or independent checkpoint-body parity.

Queue admission bounds both count and estimated payload bytes, retaining permits
for deferred input. Ordinary input and operation updates each default to 256
events / 32 MiB aggregate / 16 MiB per event. Controls have an independent reserved
budget of 16 events / 1 MiB aggregate / 64 KiB per event, so deferred prompts cannot
prevent stop or the operation updates they await. These are new configurable queue
policies, not a global RSS budget. Context, retained operation state, in-flight
snapshots and serialized records require separate accounting.

`OperationHandle::submit` acknowledges admission only. The opt-in
`submit_committed` waits until the operation has been journaled and processed by
the coordinator, forwarding contextual journal/processing failures. Discarded
updates and driver teardown close the receipt rather than inventing success.
Dropping a sender's future does not withdraw an admitted update or prove that it
was not committed. Four native tests exercise delayed/refused writes, discarded
receipts, driver/sender aborts and retention of admission through processing.

## File/value operation transitions

```sh
python3 compat/operation_parity.py
```

`kou-runtime::operation` implements the durable file/value specs, decoding and
pure state transitions. **151 exact Go/Rust projections** cover initial and
terminal states, source/result validation, cancellation, null/opaque values,
metadata preservation and restart policy. The development oracles inspect
compute dispatches but never execute them. Awaiting Read/Write may redispatch on
recovery; awaiting Edit/Patch fail with the original read-before-retry warning.
This does not prove exactly-once file effects around a crash.

Six native tests additionally exercise explicit private Read/Write/Edit effects,
completion transitions and nested result shape guards. Positional arrays and
scalar FileResults are rejected with their exact JSON pointer; null remains unset.
A file task is moved into its dispatch and a finished result is moved into the
next state, rather than deep-cloned at that boundary.
Read/Write/Edit/Patch use the existing native file backends. Transcript-search
execution remains unconnected. Malformed typed-field diagnostics and wider
state/event fuzzing are still gates.

### File/value/shell local manager

`LocalManager` runs file/value v1 and shell v3 through the same fixed worker pool
(default four, maximum eight). It deduplicates accepted IDs, preserves FIFO
snapshots, cancels queued tasks before effects and discards a late result after
cancellation. Drop closes admission and joins all workers, even with a retained
update receiver. Blocking filesystem calls are not forcibly preemptible.

Shell workers wait for each `Update::acknowledge` before crossing the next effect
boundary. The consumer must durably publish the snapshot, for example through
`OperationHandle::submit_committed`, before acknowledging success. Refusal or a
dropped unacknowledged update fails the task and cleans up its owned invocation.
Cancel/shutdown wake pending receipts and share the native cancellation flag;
there is no per-process watcher thread. They cannot authorize a subsequent effect.

New policies cap admitted tasks at 64, estimated retained reservations at 128 MiB,
and input operations at 16 MiB. File completion growth is capped at 256 KiB. Shell
snapshots instead have a checked cap of `input_bytes*8 + max_output_runes*16 +
completion_extra_bytes + 4096`, and reserve sixteen such slots for twelve normal
checkpoints, a failure, input and typed state. This conservative policy can reject
a one-million-rune shell under the default 128 MiB budget; callers must explicitly
raise the budget rather than silently lose output. Reservations allocate no
buffers. Immutable updates retain the task lease through forwarding/acknowledgement.
Seen IDs are boxed strings, capped at 100,000 IDs and 8 MiB of ID payload. Oversized
completions fail explicitly, warning that an effect may already have happened.
These bounds exclude filesystem/JSON scratch and caller-retained clones; they
are **not a global RSS guarantee** or Go-default compatibility claim.

Eleven file/value manager tests and eight shell manager tests cover admission,
shared workers, immutable snapshots, durable-receipt refusal/drop, queued/active
cancel, late success, panic recovery, count/byte/metadata guards and joined shutdown.
A real coordinator/FileJournal/manager fixture prevents file effects across failed
response/status writes. The shell fixture covers success plus response/status,
create-directory/out/err, pre-start and started-write refusal. Restoring the
recorded process/PID-zero state fails with unknown outcome without relaunching or
truncating captures. Completed delivered operations stay cold on recovery. These
are actual private fixtures, not production runner wiring or independent full
Go/Rust manager trace parity. All nineteen manager tests and the eight-case
coordinator shell fixture also passed twenty extra repetitions.

## Skill-use operation transitions

`kou-runtime::operation::{skill_spec, decode_skill, advance_skill}` implements
skill-use v1 as pure transitions. The read dispatch preserves the raw path,
`skill-use-read` correlation, saved byte offset and `i64::MAX` count. Content
stays bytes/base64, including invalid UTF-8 and split multibyte sequences, rather
than being prematurely converted to text. The first read chunk can be moved into
state. The shared Shell/Skill base64 decoder now accepts borrowed string data,
without an intermediate owned copy of the encoded string.

Ready requires initial state; it does not discard stale content. Awaiting recovery
resumes saved bytes but rejects a terminal error. Source mismatches fail the
operation. Failure/cancellation precede correlation checks; wrong read payloads,
offsets or completion sizes fail with Go's exact terminal diagnostics. Canceling
rejects an event, and terminal operations cannot advance. Whitespace-only paths
are preserved: the Go spec rejects only the empty string.

`compat/operation_parity.py` now compares **642 projections**: the existing 151
file/value cases and **491 skill-use cases**, including 80 seeded byte histories
across decode/recovery/append/completion. Dispatch path, offset and count are
compared, not just their presence. Six Rust units cover metadata, wire types,
ordered errors, recovery, byte checkpoints and failure/cancel precedence. These
fixtures execute no read dispatch. Native skill I/O, LocalManager integration,
resource limits, the SkillUse translator and registry are still open. In
particular, preserving an unbounded pure Content/error contract is not a claim
that the runner's memory is bounded. All 642 operation projections, 354 Shell
projections, 1,597 translator projections, six skill units and sixteen translator
units passed twenty additional repetitions. Go's Skill tests passed three race
repetitions, and the development operation oracle passed vet.

## Native artifact I/O

`kou-tools::artifact::create` pins the parent directory descriptor, creates a file
with `openat(EXCL)` or a directory with `mkdirat`, and validates existing leaf
entries without following symlinks. Replay neither truncates contents nor changes
permissions, including mode-000 artifacts. The parent is synced on creation and
reuse; new files are synced first. Apple platforms also use `F_FULLFSYNC` on the
parent, ignoring only `ENOTSUP`, as the existing Go helper does. Descriptor cleanup
is RAII; unlike the Go helper, individual close-system-call errors are not exposed.

`read_capture` is deliberately narrower than the general Go read primitive: a
regular file, no leaf symlink, nonnegative offset and count no greater than
4,000,000 bytes. It borrows at most 8 KiB per callback and retains no output body.
Cancellation or callback failure stops delivery. These are explicit capture
policies, not a claim of full Go I/O/error conformance or a global RSS limit.

Eight private native tests cover idempotent creation, unchanged contents/modes,
restrictive permissions, symlink/FIFO/wrong-kind refusal, exact path resolution,
24-way creation races, bounded reads, cancellation and callback errors/panics.
Non-UTF-8 filename behavior is compared with the host filesystem itself: APFS
rejects those names rather than silently repairing them. The native shell executor
uses these helpers. Independent Go/Rust artifact I/O traces and an exhaustive
filesystem-failure matrix remain open.

## Shell v3 transitions

```sh
python3 compat/shell_parity.py
```

`operation::shell::Shell` owns decoded durable state and a bounded ephemeral read
accumulator. Returned checkpoints precede artifact creation, process start and
read dispatches; chunks alone do not create journal records. Resume of an unknown
or interrupted process fails closed without relaunching or signaling a saved PID.
Read-only phases can redispatch. Completion frees inline buffer capacities and
retains bounded rune-based head/tail text, original byte counts and spool paths.

**354 Go/Rust transition projections** compare every phase, malformed sources and
results, restart/cancel behavior, prefix/tail assembly, binary UTF-8 repair,
base64/null fields and output/error clipping. Only Go's deliberate `cannot` /
`unable to` JSON modal variants are accepted; all remaining diagnostic bytes and
opaque data stay exact. Go's JSON byte decoder rejects CR/LF although its base64
package alone permits them. This distinction is reproduced, not normalized away.

The Go shell reducer can send a signal itself when a timer fires. The oracle
rejects every fired-timer fixture with an unowned process group before invoking
it; returned dispatches are never executed. The Rust reducer instead returns a
signal dispatch. The native executor validates it against its currently owned
invocation, never a recovered numeric PID. Six native tests verify pure signal
requests, explicit nanosecond clocks/deadlines/duration, bounded chunk recovery,
typed-object guards and a real private session journal preceding a 200 KB native
process capture and prefix/tail reads. That fixture executes dispatches **only in
test code**. Clock-dependent branches are tested natively, not presented as exact
Go comparisons by stripping their timestamps.

### Native shell executor

`run_shell` runs the typed actor and native artifact/capture backends on its
caller-owned worker, without creating threads. It emits each checkpoint before
its dispatch, and treats a publisher error as a refusal to cross that boundary.
This is a durable acknowledgement only when the publisher actually journals and
acknowledges the write; queue admission alone is not durability. The executor
limits an invocation to twelve checkpoints. Native failures become failed states;
cancellation before an effect or after native completion wins over late success.

Seven actual-journal tests cover complete 380 KB output on disk with bounded
inline results, refused create/start boundaries, refused/panicking started
publication and owned cleanup, timeout publication before TERM, cancellation,
spawn/read failures and post-exit read recovery. Two units verify monotonic
in-flight duration across a backwards wall-clock jump and Go-compatible
nanosecond truncation of fractional timeouts. Five capture-event tests cover
callback refusal/panic, cancellation, invalid timer configurations, timeout-before-
TERM ordering and a slow timeout publisher that must not consume physical TERM
grace. Callbacks are synchronous and non-preemptible; spool disk is uncapped.

The bounded local manager now uses the concrete executor and explicit durable
receipts. Production runner wiring and persistent process status/attachment/control
remain open.

## Model-call translators

`kou-runtime::tool::Bash` is a pure production translator from model arguments to
shell v3 specs and from immutable operation snapshots to tool results. It preserves
Go's output-limit/timeout/command validation order, default/null timeout semantics,
NUL byte offsets, original command whitespace, stdout/stderr separators, nonzero
exits, timeout/duration notices, capture paths and running/failed/canceled states.
Translation records specs; it never executes a command or reads a capture.

Arguments borrow raw JSON values rather than retaining an owned arguments tree.
The shared numeric decoder accepts Go-style decimal, underscore and hexadecimal
numeric strings, rejects blank/non-finite values and performs one ties-to-even
IEEE rounding including subnormals. An exact-only hex parser was unsuitable, so
this is a small safe streaming-significand decoder, not an allocator dependency.
Three native tests cover its edge rounding, clipped validation errors, unchanged
command/fractional timeout, explicit running clocks and preserved output newlines.

`kou-runtime::tool::File` supplies Read/Edit/Write/Patch/transcript-search call
and result translation, including aliases. It decodes every known typed argument
in source order, even fields unused by that kind. Unescaped strings borrow the
input and only fields used by the spec become owned. Relative paths use the exact
workspace + slash + path spelling, preserving doubled slashes and `..`. Numeric
counts follow Go's exact-whole-number range through 2^53 before negative checks;
the same boundary now preserves Bash output-limit diagnostics too. Completed file
text is moved into the result, without trimming or an extra full-text clone.

Read's per-instance repetition journal keeps keyed text digests, not read bodies.
Defaults cap it at **1,024 parts, 4,096 call IDs and 1 MiB of key payload**. Shared
Arc keys and a FIFO bound retained entries; zero disables tracking and oversized
keys are not cached. Repeated call IDs retain their saved count. Changed text
resets a part's count; raw path/offset/limit identify distinct parts. Eviction
forgets old history, an explicit new policy rather than equivalence to Go's
unbounded maps or a global RSS guarantee. Nine File/journal units cover these
contracts, borrowing, source-order errors, large bodies, caps and 32 concurrent
read results. The journal reuses the state decoded for result formatting.

`kou-runtime::tool::Command` translates model-visible plugin commands into the same
shell v3 specs. It preserves argv, exact workspace/base paths, sorted exports,
reserved tool/call-ID overrides, literal single-quote escaping and compact raw
JSON on standard input. The here-document receives one JSON line even when a
string contains escaped newlines or its delimiter. It borrows the raw JSON span
and writes it into the script buffer without building an owned value tree or
cloning the environment. Configured output limits and `waits` are preserved;
result text reuses the Bash formatter, while the running description remains
empty as in Go. `Translator::translate_call` carries the original call identity;
the argument-only method remains available for calls without an ID.

Four units cover constructors, scripts/identity/quoting, validation limits and
result formatting. An actual private Command + LocalManager + FileJournal test
runs success and refused-process cases. It verifies that refused durability
cannot execute the command, reserved variables override supplied values, quoted
arguments/call IDs cannot inject shell code, and stdin/cwd/captures/result text
are exact. The capture base must already exist, as required by the native
artifact backend. This is not the full plugin registry/loader or Code host path.

`compat/translator_parity.py` compares **1,597 pure Go/Rust projections**: 407 Bash,
930 File (including **54 stateful sequences / 1,406 result steps**) and 260 Command.
It retains the 96 seeded hex values and 80 seeded Bash calls, and adds all five
file kinds, aliases, strict roots/known-field types, source-order/count/path errors,
result headers/status/text, nested result shapes and seeded Read histories. The Go
oracle records specs and executes none. Real-time elapsed branches use explicit
native clocks, not stripped Go timestamps. The only accepted diagnostic variation
is Go JSON v2's source-proven `cannot`/`unable to` modal in known error fields,
including sequence children. Full diagnostics otherwise match exactly; clipped
Bash messages are reconstructed from the original token and independently bounded.
Changed counts, context, pointers and opaque payloads are rejected by self-tests.
All 1,597 translator projections, 151 operation projections, sixteen translator
units, six operation units and the actual Command/journal fixture passed twenty
additional repetitions. Go Bash/Command/File/toolarg passed three race-test
repetitions plus vet.

This slice does not yet supply the full runtime registry or Code translator, all
malformed typed-field/JSON/config/operation diagnostics or a whole-runner
performance measurement. Transcript-search execution is still unconnected.
No Go helper is required by the Rust production translators.

## Local validation and remaining work

The latest complete local gate passed **306 native workspace tests**, including
**68 in kou-engine**, all **fifteen compatibility suites**, fmt/clippy, WASM,
Python/JS syntax and diff checks. Go coordinator/contextbuilder/operation, primitive
and Bash/Command/File/toolarg race tests passed; development-oracle vet checks passed.
Inventory pins **94 sources, 136 HTTP registrations and 31 built-in plugins**.

The live core is not the production runner. Persistent process controls, the full
Code runtime, remaining result translators, Code wire/reporting/images/search, stream-event reporting,
runner auth/CLI/detached lifecycle and wider live timing/crash compatibility
remain open. Default local-time notice formatting also needs timezone-abbreviation
compatibility review. No whole-runner CPU/RSS advantage has been measured.

The local root plan/status reports describe the remaining approved target.
Do not replace the running Go installation or enable Rust writes to user sessions
until mixed-version durability, execution and client compatibility gates pass.
Full server API, WASM cockpit/plugins, accounts gateway, canvas/PTY/voice and relay
remain outstanding. No GPUI.
