#!/usr/bin/env python3
"""Generate a source-pinned contract inventory; never reads sessions or secrets."""
import hashlib
import json
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parents[1]
SOURCES = [
    "cmd/kou-conveyor-web/server.go",
    "cmd/kou-conveyor-web/gatewaypeer.go",
    "cmd/internal/canvas/http.go",
    "cmd/internal/tunnel/tunnel.go",
    "cmd/internal/cockpit/detached.go",
    "cmd/internal/cockpit/process_unix.go",
    "cmd/internal/agentrunner/run.go",
    "cmd/internal/agentrunner/providers.go",
    "cmd/internal/accounts/accounts.go",
    "harness/sessionstore/localfile/codec.go",
    "harness/sessionstore/localfile/state.go",
    "harness/sessionstore/localfile/checkpoint.go",
    "harness/codevm/codevm.go",
    "harness/codevm/exec.go",
    "harness/codevm/tools.go",
    "harness/codevm/declarations.go",
    "harness/fileops/fileops.go",
    "harness/fileops/patch.go",
    "harness/fileops/lock.go",
    "harness/primitives/process.go",
    "internal/spawn/spawn.go",
    "harness/sessionstore/itemjson.go",
    "harness/llm/itemjson.go",
    "harness/llm/model.go",
    "harness/llm/stream.go",
    "harness/llm/responsesapi/adapter.go",
    "harness/llm/responsesapi/request.go",
    "harness/llm/responsesapi/response.go",
    "harness/llm/responsesapi/stream.go",
    "harness/llm/responsesapi/retry.go",
    "harness/llm/responsesapi/images.go",
    "harness/llm/overflow.go",
    "harness/llm/ratelimit.go",
    "harness/llm/retry.go",
    "harness/llm/warn.go",
    "harness/llm/clients/openai/client.go",
    "harness/llm/clients/fireworks/client.go",
    "harness/llm/clients/openrouter/client.go",
    "harness/llm/clients/ollama/client.go",
    "harness/llm/clients/openaicodex/client.go",
    "harness/llm/clients/openaicodex/credentials.go",
    "harness/llm/clients/anthropic/client.go",
    "harness/llm/clients/anthropic/request.go",
    "harness/llm/clients/anthropic/response.go",
    "harness/llm/clients/anthropic/prefix.go",
    "harness/llm/prefix.go",
    "harness/llm/model.go",
    "harness/contextbuilder/contextbuilder.go",
    "harness/contextbuilder/builder.go",
    "harness/contextbuilder/checkpoint.go",
    "harness/contextbuilder/compaction.go",
    "harness/contextbuilder/images.go",
    "harness/contextbuilder/input.go",
    "harness/contextbuilder/linked.go",
    "harness/contextbuilder/mask.go",
    "harness/contextbuilder/memory.go",
    "harness/contextbuilder/prune.go",
    "harness/contextbuilder/skills.go",
    "harness/contextbuilder/prompts/preamble.md",
    "harness/contextbuilder/prompts/compaction.md",
    "harness/contextbuilder/prompts/compacted.md",
    "harness/contextbuilder/prompts/skill-preamble.md",
    "harness/inbox/inbox.go",
    "harness/tool/registry.go",
    "harness/tool/tool.go",
    "harness/coordinator/coordinator.go",
    "harness/coordinator/loop.go",
    "harness/coordinator/checkpoint.go",
    "harness/operation/operation.go",
    "harness/operation/file.go",
    "harness/operation/value.go",
    "harness/operation/shell.go",
    "harness/operation/output.go",
    "harness/primitives/file.go",
    "harness/primitives/file_create_unix.go",
    "harness/primitives/file_create_sync_darwin.go",
    "harness/primitives/file_create_sync_unix.go",
    "harness/operation/local_manager.go",
    "harness/primitives/compute.go",
    "harness/primitives/primitive_dispatch.go",
    "harness/primitives/primitive_event.go",
    "harness/primitives/remote.go",
    "harness/primitives/sse.go",
    "harness/primitives/remote_sse.go",
    "harness/plugin/plugin.go",
    "docs/plugins.md",
    "ios/KouConveyor/Core/RelayClient.swift",
]


def inventory():
    sources = []
    routes = []
    for name in SOURCES:
        data = (ROOT / name).read_bytes()
        sources.append({"file": name, "sha256": hashlib.sha256(data).hexdigest()})
        for number, line in enumerate(data.decode().splitlines(), 1):
            if "HandleFunc(" in line or "mux.Handle(" in line:
                routes.append({"file": name, "line": number, "registration": line.strip()})
    plugins = []
    for manifest in sorted((ROOT / "cmd/kou-conveyor-web/plugins").glob("*/plugin.json")):
        value = json.loads(manifest.read_text())
        plugins.append({"id": value.get("id", manifest.parent.name), "manifest": str(manifest.relative_to(ROOT)), "web": value.get("web", {})})
    return {
        "version": 1,
        "kind": "source-inventory-not-an-API-conformance-claim",
        "sources": sources,
        "http_registrations": routes,
        "builtin_plugins": plugins,
        "storage": {"format": 2, "checkpoint_envelope": 1, "record_types": ["session", "item", "operation", "checkpoint"]},
        "migration_gates": {
            "store_archive_parity": "compat/store_parity.py",
            "consumer_checkpoint_resume": "store envelope and context-builder checkpoints pass bidirectional Go/Rust checks; compat/replay_parity.py adds 138 actual Go coordinator replay scenarios with scalar-state/operation-header probes and 19 native coordinator checkpoint import/fallback matrices; native private store fast/full/invalid-consumer fallback and freshness/session guards pass; full runner integration remains open",
            "async_coordinator": "42 native driver tests cover durable boundaries/errors, ordered reconciliation/deferred prompts, non-steering controls, stop/cancel, grace/admission, verification, backoff and budgets; optional submit_committed waits for journal+processing acknowledgement, forwarding failures without confusing admission with durability; four receipt tests cover delayed/refused writes, sender/driver abort, discarded updates and retained admission; compat/loop_parity.py compares 44 actual Go/Rust live traces with private real journals and inert ports; actual HTTP/SSE, private FileJournal restart/summary matrix and real native file-manager durable-boundary fixture pass; production runner wiring and exhaustive live timing/crash coverage remain open",
            "context_builder": "compat/context_parity.py: 285 exact request/report/estimate/memory/compaction scenarios including bidirectional checkpoint import, 82 image-header cases, and 80 seeded histories; native Arc snapshots/prompt budgets tested; no providers",
            "context_schema_fuzz": "malformed typed-field/array diagnostics and broader image/checkpoint shape fuzzing remain release gates",
            "writer_append_fork_checkpoint_lock_parity": "compat/writer_parity.py (temporary fixtures only)",
            "writer_partial_io_crash_matrix": "partial: poisoned-descriptor unit test, exhaustive crash points still open",
            "code_vm_semantic_projection": "compat/codevm_parity.py (fake plugins; not full Result wire conformance)",
            "native_file_tools": "compat/fileops_parity.py and compat/patch_parity.py (temporary fixtures, bounded inputs)",
            "native_vm_tools": "compat/native_parity.py (real temporary file/process/plugin capabilities, no providers)",
            "native_process_lifecycle": "kou-tools tests cover groups/termination/escalation/pipes/CLOEXEC plus eight direct-file capture cases and five lifecycle callback cases including slow timeout publication preserving physical TERM grace; compat/capture_parity.py compares 19 owned Go/Rust captures, exact bytes/modes/raw exit/signal/cancellation and cleanup; output bodies stay on disk, spool disk is uncapped; not full primitive controls, persistent process or sandbox service",
            "native_artifact_io": "eight private native tests: parent-descriptor-relative exclusive creation, no-follow existing-kind validation, non-destructive replay/zero modes, sync/new-file+parent and Apple fullfsync, exact path resolution and 24-way races; capture read regular/no-follow/count<=4MB/8KiB borrowed chunks/cancellation/callback failure; used by the native shell executor; no independent Go/Rust artifact trace or exhaustive filesystem fault matrix, RAII close errors not surfaced",
            "shell_operations": "compat/shell_parity.py compares 354 pure shell v3 transitions; six native reducer tests plus seven actual-journal executor tests and two clock/timeout units cover bounded head/tail, restart, checkpoint-before-effect, refusal/panic cleanup, timeout-before-TERM, cancellation and post-exit recovery; native run_shell creates no threads, max twelve checkpoints, durable ACK only when publisher supplies it; Go oracle rejects timer signals to unowned PIDs, clock branches are not normalized into fake parity; bounded manager integration, production runner and persistent process controls remain open",
            "file_value_operations": "compat/operation_parity.py compares 135 exact pure transitions with Go handlers; five native effect/transition tests plus eleven local-manager tests (20 extra repeats), fixed workers, estimated count/byte/ID bounds, cancel, immutable updates, panic recovery and joined shutdown; actual coordinator/journal fixture prevents effects across failed response/status writes; production runner, persistent process, transcript search and independent full manager traces remain open",
            "responses_codec_sse_http": "compat/provider_parity.py (433 cases, 161 real Go/Rust local HTTP exchanges, 89 synthetic Codex credential/config cases), plus cancellation/deadline/gzip/admission native tests; no paid calls",
            "responses_provider_clients": "OpenAI/Fireworks/OpenRouter/Ollama/Codex implemented and compared; runner auth integration remains",
            "anthropic_messages": "compat/anthropic_parity.py (355 codec/SSE/HTTP/listing cases); native byte/depth bounds, streaming prefix fingerprints, cancellation/deadline/drop/admission/ping/pagination tests; no paid calls",
            "anthropic_sdk_edges": "full SDK malformed/duplicate/rare union shapes and resource-policy compatibility remain release gates; no provider RSS/CPU advantage measured",
            "responses_resource_and_live_tls": "not measured end-to-end; configurable new request/frame/retention/cache caps and TLS handshake timeout differences require release review",
            "runner_execution_parity": "not_implemented",
            "browser_plugin_API_2": "not_implemented",
            "gateway_support_matrix": "not_implemented",
            "end_to_end_resource_baseline": "not_measured",
        },
    }


def main():
    import argparse
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--check", action="store_true")
    args = parser.parse_args()
    destination = ROOT / "compat/contracts.json"
    text = json.dumps(inventory(), ensure_ascii=False, indent=2) + "\n"
    if args.check:
        if not destination.exists() or destination.read_text() != text:
            raise SystemExit("contract source inventory is stale; run python3 compat/inventory.py")
    else:
        destination.write_text(text)
    value = json.loads(text)
    print(f"{len(value['sources'])} pinned sources; {len(value['http_registrations'])} HTTP registrations; {len(value['builtin_plugins'])} built-in plugins")


if __name__ == "__main__":
    main()
