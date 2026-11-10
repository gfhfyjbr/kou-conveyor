#!/usr/bin/env python3
"""Build isolated kernels, validate parity, and run interleaved fresh-process probes."""
from __future__ import annotations

import argparse
import hashlib
import json
import math
import os
from pathlib import Path
import random
import re
import shutil
import statistics
import subprocess
import sys
import time

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
PREFIX = "rust-go"
MANIFEST = ROOT / f"{PREFIX}-fixtures.json"
RESULTS = ROOT / f"{PREFIX}-results.json"


def dump(path, value):
    path = Path(path)
    with path.open("w", encoding="utf-8") as file:
        os.chmod(path, 0o600)
        json.dump(value, file, ensure_ascii=False, indent=2)
        file.write("\n")


def environment(**extra):
    env = dict(os.environ)
    for key in ("KOU_CANVAS_TOKEN", "KOU_CONVEYOR_TUNNEL_CONFIG", "GOMEMLIMIT", "MALLOC_CONF", "_RJEM_MALLOC_CONF"):
        env.pop(key, None)
    for key in list(env):
        if key.startswith("RUSTGO_"):
            env.pop(key)
    sdk = Path("/Library/Developer/CommandLineTools/SDKs/MacOSX15.2.sdk")
    if sys.platform == "darwin" and sdk.is_dir():
        env["SDKROOT"] = str(sdk)
        env["CC"] = subprocess.check_output(["xcrun", "--find", "clang"], text=True).strip()
    # Go uses PWD when it names packages; keep overlays correct through workspace symlinks.
    env["PWD"] = str(ROOT)
    env["GOGC"] = "50"
    env.update({key: str(value) for key, value in extra.items()})
    return env


def call(command, *, env=None, log=None):
    result = subprocess.run([str(x) for x in command], cwd=ROOT, env=env or environment(), text=True, capture_output=True)
    if log:
        Path(log).write_text(result.stdout + "\nSTDERR:\n" + result.stderr)
    if result.returncode:
        raise RuntimeError(f"command failed ({result.returncode}): {' '.join(map(str, command))}\n{result.stdout[-4000:]}\n{result.stderr[-6000:]}")
    return result


def payload(output):
    lines = [line[len("RUSTGO "):] for line in output.splitlines() if line.startswith("RUSTGO ")]
    if len(lines) != 1:
        raise RuntimeError(f"expected one probe row, got {len(lines)}: {output[-1000:]}")
    return json.loads(lines[0])


def build(args):
    overlay = {"Replace": {
        str(ROOT / "cmd/internal/cockpit/zz_rust_go_probe_test.go"): str(HERE / "go/cockpit_probe_test.go.txt"),
        str(ROOT / "cmd/internal/cockpit/zz_rust_go_batch_test.go"): str(HERE / "go/cockpit_batch_test.go.txt"),
        str(ROOT / "harness/primitives/zz_rust_go_probe_test.go"): str(HERE / "go/primitives_probe_test.go.txt"),
    }}
    dump(ROOT / f"{PREFIX}-overlay.json", overlay)
    for name, package in (("cockpit", "./cmd/internal/cockpit"), ("primitives", "./harness/primitives")):
        call(["go", "test", "-c", "-overlay", ROOT / f"{PREFIX}-overlay.json", "-o", ROOT / f"{PREFIX}-{name}.test", package], log=ROOT / f"{PREFIX}-build-go-{name}.txt")
        listed = call([ROOT / f"{PREFIX}-{name}.test", "-test.list=RustGo"]).stdout
        required = "TestRustGoCockpitProbe" if name == "cockpit" else "TestRustGoSSEProbe"
        if required not in listed:
            raise RuntimeError(f"overlay did not inject {required}; check workspace PWD/symlink resolution")
    variants = [("system", []), ("system-counted", ["count-alloc"])]
    if not args.no_jemalloc:
        variants += [("jemalloc", ["jemalloc"]), ("jemalloc-counted", ["jemalloc", "count-alloc"])]
    for name, features in variants:
        print(f"building {name}", flush=True)
        command = ["cargo", "build", "--release", "--locked", "--manifest-path", HERE / "Cargo.toml",
                   "--target-dir", HERE / "target"]
        if features:
            command += ["--features", ",".join(features)]
        call(command, log=ROOT / f"{PREFIX}-build-{name}.txt")
        shutil.copy2(HERE / "target/release/kou-rust-go-probe", ROOT / f"{PREFIX}-{name}.test")
    print("build complete", flush=True)


def rich_snapshot(count):
    rows = []
    for i in range(count):
        entry = {"id": f"entry-{i:08d}", "kind": "tool" if i % 2 else "assistant", "at": "2026-11-09T09:00:00.123456789Z",
                 "text": (f"entry {i}: Привет 🦀\n" + "text with < > & and a tab\t") * (1 + i % 5), "model": "test-model"}
        if i % 2:
            entry["tool"] = {
                "call_id": f"call-{i:08d}", "name": "Code", "input": f"file-{i}.go", "state": "done", "exit_code": 0,
                "output": "output\n" * (20 + i % 100), "stderr": "warning" if i % 7 == 0 else "",
                "started": "2026-11-09T09:00:00Z", "finished": "2026-11-09T09:00:00.1Z",
                "syntax": [5, 1, 7, 2] * 16,
                "calls": [{"name": "Read", "input": "file.go", "gist": "file.go", "output": "result", "state": "done", "exit_code": 0} for _ in range(3)],
                "logs": "console line", "value": "42", "files": ["file.go", "other.rs"],
            }
            if i % 3 == 0:
                entry["tool"]["image"] = {"label": "image", "media_type": "image/png", "width": 320, "height": 240, "size": 12000}
                entry["tool"]["change"] = {"kind": "edit", "diff": "@@ -1 +1 @@\n-old\n+new", "line": 1, "replacements": 1, "syntax": [5, 2, 4, 1]}
        if i % 11 == 0:
            entry.update({"forced": True, "gone": True, "replaces": "preview", "detail": "details", "phase": "commentary", "state": "pending"})
            entry["images"] = [{"label": "[Image 1]", "media_type": "image/png", "width": 100, "height": 200, "size": 1200}]
            entry["files"] = [{"label": "$file", "path": "file.go", "from": 1, "to": 10, "lines": 50, "size": 4096}]
        rows.append(entry)
    return rows


def describe(path, name):
    path = Path(path)
    digest = hashlib.sha256()
    with path.open("rb") as file:
        for block in iter(lambda: file.read(1024 * 1024), b""):
            digest.update(block)
    return {"name": name, "path": str(path), "bytes": path.stat().st_size, "sha256": digest.hexdigest()}


def prepare(args):
    manifest = json.loads(MANIFEST.read_text()) if args.append and MANIFEST.exists() else {"snapshots": [], "scans": []}
    if not args.append:
        for name, count in (("rich-small", 128), ("rich-large", 4096)):
            path = ROOT / f"{PREFIX}-{name}.json"
            dump(path, rich_snapshot(count))
            manifest["snapshots"].append(describe(path, name))
        path = ROOT / f"{PREFIX}-sse-input.json"
        path.write_bytes((b"data: " + b"x" * 120 + b"\n\n") * 256)
        os.chmod(path, 0o600)
        manifest["sse"] = describe(path, "sse-256")
        cases = ROOT / f"{PREFIX}-sse-cases.json"
        result = call([ROOT / f"{PREFIX}-primitives.test", "-test.run=^TestRustGoSSEExport$", "-test.v"], env=environment(RUSTGO_OUTPUT=cases))
        print(result.stdout.strip(), flush=True)
        manifest["sse_cases"] = describe(cases, "differential")
        edges = ROOT / f"{PREFIX}-scan-edges.json"
        external = lambda at: {"type": "item", "data": {"Item": {"Kind": "input", "RecordedAt": at, "Data": {"Kind": "external"}}}}
        lines = [external("2026-11-09T12:00:00+03:00"), {"type": "operation", "data": {"out": "x" * 200000}}, external("2026-11-09T09:00:00.000000001Z")]
        edges.write_text("\n".join(json.dumps(x, separators=(",", ":")) for x in lines) + "\nmalformed\n" + json.dumps(external("2099-01-01T00:00:00Z"), separators=(",", ":")))
        os.chmod(edges, 0o600)
        manifest["scan_edges"] = describe(edges, "scan-edges")
        scan = ROOT / f"{PREFIX}-scan-synthetic.json"
        with scan.open("wb") as file:
            os.chmod(scan, 0o600)
            for i in range(8192):
                record = external(f"2026-11-09T09:{i//60%60:02d}:{i%60:02d}Z") if i % 128 == 0 else {"type": "operation", "data": {"output": "x" * 1000}}
                file.write(json.dumps(record, separators=(",", ":")).encode() + b"\n")
        manifest["scans"].append(describe(scan, "synthetic-8MiB"))
    existing = {row["name"] for row in manifest["scans"]}
    for index, source in enumerate(args.session):
        source = Path(source).resolve()
        before = source.stat()
        name = f"real-{source.stem[:8]}"
        if name in existing:
            continue
        scan = ROOT / f"{PREFIX}-{name}-log.json"
        with source.open("rb") as src, scan.open("wb") as dst:
            os.chmod(scan, 0o600)
            shutil.copyfileobj(src, dst)
        after = source.stat()
        if (before.st_size, before.st_mtime_ns) != (after.st_size, after.st_mtime_ns):
            raise RuntimeError("source session changed during copy; choose an inactive session")
        manifest["scans"].append(describe(scan, name))
        snapshot = ROOT / f"{PREFIX}-{name}-snapshot.json"
        result = call([ROOT / f"{PREFIX}-cockpit.test", "-test.run=^TestRustGoExportSnapshot$", "-test.v"], env=environment(RUSTGO_SESSION=source, RUSTGO_OUTPUT=snapshot))
        print(result.stdout.strip(), flush=True)
        if (after.st_size, after.st_mtime_ns) != (source.stat().st_size, source.stat().st_mtime_ns):
            raise RuntimeError("source session changed during export; choose an inactive session")
        manifest["snapshots"].append(describe(snapshot, name))
    dump(MANIFEST, manifest)
    print(json.dumps({key: [(row["name"], row["bytes"]) for row in manifest[key]] for key in ("snapshots", "scans")}, indent=2), flush=True)


def go_probe(mode, path, iterations, gc=50):
    package = "primitives" if mode.startswith("sse") else "cockpit"
    test = "TestRustGoSSEProbe" if package == "primitives" else "TestRustGoCockpitProbe"
    return [ROOT / f"{PREFIX}-{package}.test", f"-test.run=^{test}$"], environment(RUSTGO_MODE=mode, RUSTGO_INPUT=path, RUSTGO_ITERATIONS=iterations, GOGC=gc)


def rust_probe(variant, mode, path, iterations):
    return [ROOT / f"{PREFIX}-{variant}.test", mode, path, iterations], environment()


def validate(args):
    manifest = json.loads(MANIFEST.read_text())
    variants = [name for name in ("system", "jemalloc", "system-counted", "jemalloc-counted") if (ROOT / f"{PREFIX}-{name}.test").exists()]
    checks = []
    result = call([ROOT / f"{PREFIX}-cockpit.test", "-test.run=^TestRustGoReuseMatchesOriginal$", "-test.v"], env=environment(RUSTGO_CHECK=1))
    print(result.stdout.strip(), flush=True)
    for variant in variants:
        row = payload(call(rust_probe(variant, "sse-check", manifest["sse_cases"]["path"], 1)[0]).stdout)
        checks.append({"variant": variant, **row})
    for fixture in manifest["snapshots"]:
        go_output = ROOT / f"{PREFIX}-check-go.json"
        call([ROOT / f"{PREFIX}-cockpit.test", "-test.run=^TestRustGoSnapshotCheck$"], env=environment(RUSTGO_MODE="clone-check", RUSTGO_INPUT=fixture["path"], RUSTGO_OUTPUT=go_output))
        expected = json.loads(go_output.read_text())
        batch_output = ROOT / f"{PREFIX}-check-go-batch.json"
        result = call([ROOT / f"{PREFIX}-cockpit.test", "-test.run=^TestRustGoSnapshotCheck$"], env=environment(RUSTGO_MODE="clone-batch-check", RUSTGO_INPUT=fixture["path"], RUSTGO_OUTPUT=batch_output))
        assert payload(result.stdout)["mode"] == "clone-batch-check"
        assert json.loads(batch_output.read_text()) == expected, (fixture["name"], "Go batch parity")
        checks.append({"fixture": fixture["name"], "variant": "go50-batch", "entries": len(expected), "parity": "public wire fields + independent mutable buffers"})
        for variant in variants:
            output = ROOT / f"{PREFIX}-check-rust.json"
            cmd, env = rust_probe(variant, "clone-check", fixture["path"], 1)
            env["RUSTGO_OUTPUT"] = str(output)
            call(cmd, env=env)
            actual = json.loads(output.read_text())
            if actual != expected:
                for i, (left, right) in enumerate(zip(expected, actual)):
                    if left != right:
                        raise AssertionError(f"snapshot mismatch: {fixture['name']}, {variant}, entry {i}; see private check files")
                raise AssertionError("snapshot length mismatch")
            checks.append({"fixture": fixture["name"], "variant": variant, "entries": len(actual), "parity": "public wire fields + independent mutable buffers"})
        print(f"snapshot parity: {fixture['name']}, {len(expected)} entries", flush=True)
    for fixture in manifest["scans"] + [manifest["scan_edges"]]:
        cmd, env = go_probe("scan", fixture["path"], 1)
        row = payload(call(cmd, env=env).stdout)
        expected = (row["last_seconds"], row["last_nanos"])
        cmd, env = go_probe("scan-reuse", fixture["path"], 1)
        row = payload(call(cmd, env=env).stdout)
        assert (row["last_seconds"], row["last_nanos"]) == expected
        for variant in variants:
            for mode in ("scan", "scan-owned"):
                cmd, env = rust_probe(variant, mode, fixture["path"], 1)
                row = payload(call(cmd, env=env).stdout)["result"]
                assert (row["last_seconds"], row["last_nanos"]) == expected, (fixture["name"], variant, mode)
        checks.append({"fixture": fixture["name"], "last_seconds": expected[0], "last_nanos": expected[1], "parity": "last external input timestamp"})
        print(f"scan parity: {fixture['name']}", flush=True)
    for mode in ("sse", "sse-chunked"):
        cmd, env = go_probe(mode, manifest["sse"]["path"], 1)
        expected = payload(call(cmd, env=env).stdout)["summary"]
        for variant in variants:
            cmd, env = rust_probe(variant, mode, manifest["sse"]["path"], 1)
            assert payload(call(cmd, env=env).stdout)["result"]["summary"] == expected
    dump(ROOT / f"{PREFIX}-validation.json", {"checks": checks, "timestamp": time.time()})
    print(f"validation passed: {len(checks)} checks", flush=True)


def timed(command, env):
    result = call(["/usr/bin/time", "-l", *command], env=env)
    row = payload(result.stdout)
    for field, pattern in (
        ("max_rss_bytes", r"(\d+)\s+maximum resident set size"),
        ("peak_footprint_bytes", r"(\d+)\s+peak memory footprint"),
    ):
        match = re.search(pattern, result.stderr)
        if match:
            row[field] = int(match[1])
    match = re.search(r"([\d.]+)\s+real\s+([\d.]+)\s+user\s+([\d.]+)\s+sys", result.stderr)
    if match:
        row.update(process_wall_seconds=float(match[1]), process_user_seconds=float(match[2]), process_sys_seconds=float(match[3]))
    return row


def benchmark(args):
    manifest = json.loads(MANIFEST.read_text())
    jobs = []
    for fixture in manifest["scans"]:
        jobs.append(("scan", fixture, [("go50", "scan"), ("go100", "scan"), ("go50-reuse", "scan-reuse"), ("system", "scan-owned"), ("system", "scan"), ("jemalloc", "scan")]))
    for fixture in manifest["snapshots"]:
        jobs.append(("snapshot", fixture, [("go50", "clone"), ("go100", "clone"), ("go50-batch", "clone-batch"), ("system", "clone"), ("system", "arena"), ("system", "arena-fresh"), ("jemalloc", "clone"), ("jemalloc", "arena")]))
    for mode in ("sse", "sse-chunked"):
        jobs.append((mode, manifest["sse"], [("go50", mode), ("go100", mode), ("system", mode), ("jemalloc", mode)]))
    if args.only:
        jobs = [job for job in jobs if args.only in job[0] or args.only in job[1]["name"]]
    rows = []
    counted_rows = []
    calibration = []
    randomizer = random.Random(8361)
    for workload, fixture, choices in jobs:
        choices = [(variant, mode) for variant, mode in choices if variant.startswith("go") or (ROOT / f"{PREFIX}-{variant}.test").exists()]
        settings = []
        for variant, mode in choices:
            make = lambda n: go_probe(mode, fixture["path"], n, gc=100 if variant == "go100" else 50) if variant.startswith("go") else rust_probe(variant, mode, fixture["path"], n)
            cmd, env = make(2)
            row = payload(call(cmd, env=env).stdout)
            per_op = row["elapsed_ns"] / row["iterations"] / 1e9
            maximum = 200 if workload == "scan" else 20000 if workload == "snapshot" else 200000
            iterations = max(2, min(maximum, math.ceil(args.seconds / max(per_op, 1e-8))))
            settings.append((variant, mode, iterations))
            calibration.append({"workload": workload, "fixture": fixture["name"], "variant": variant, "mode": mode, "iterations": iterations, "sample_ns_per_op": per_op * 1e9})
        for repeat in range(args.runs):
            order = list(settings)
            randomizer.shuffle(order)
            for variant, mode, iterations in order:
                cmd, env = go_probe(mode, fixture["path"], iterations, gc=100 if variant == "go100" else 50) if variant.startswith("go") else rust_probe(variant, mode, fixture["path"], iterations)
                row = timed(cmd, env)
                row.update(workload=workload, fixture=fixture["name"], variant=variant, repeat=repeat)
                rows.append(row)
                print(f"{workload:11} {fixture['name']:18} {variant:12} {mode:12} {row['elapsed_ns']/iterations/1e6:9.4f} ms/op  peak {row.get('max_rss_bytes',0)/2**20:7.2f} MiB", flush=True)
            dump(RESULTS, {"rows": rows, "counted_rows": counted_rows, "calibration": calibration, "runs": args.runs, "target_seconds": args.seconds, "notes": notes()})
        for variant, mode, _ in settings:
            if variant.startswith("go"):
                continue
            counted = variant + "-counted"
            if not (ROOT / f"{PREFIX}-{counted}.test").exists():
                continue
            cmd, env = rust_probe(counted, mode, fixture["path"], 10)
            row = payload(call(cmd, env=env).stdout)
            row.update(workload=workload, fixture=fixture["name"], variant=variant)
            counted_rows.append(row)
        dump(RESULTS, {"rows": rows, "counted_rows": counted_rows, "calibration": calibration, "runs": args.runs, "target_seconds": args.seconds, "notes": notes()})
    summarize(args)


def notes():
    return [
        "Timing and process peaks use uninstrumented Rust binaries; Rust allocation counts use separate count-alloc builds.",
        "Go allocation bytes are runtime size-class accounted; Rust counted bytes are requested Layout bytes, not malloc usable sizes.",
        "max RSS and peak footprint cover the whole fresh process, including startup, fixture parsing and warmup, not just the timed loop.",
        "The loop uses warm filesystem cache; lastPrompt cold means reset logical scan cache, not cold disk.",
        "Snapshot port covers public Entry DTO only; it does not port private fold state, Rev, Transcript indices or coordinator semantics.",
        "Arena snapshots borrow immutable text from the source and must finish before the source is mutated/dropped; owned clones have no such borrow restriction.",
        "Arena mode resets a reusable arena per request; arena-fresh creates and drops a new arena per request.",
        "Go GOGC=50 matches web default; GOGC=100 is a separate control. GOMEMLIMIT is unset. GOMAXPROCS is not overridden.",
        "No production process restarts, LLM requests or source-session writes are performed.",
    ]


def summarize(args):
    data = json.loads(RESULTS.read_text())
    groups = {}
    for row in data["rows"]:
        key = (row["workload"], row["fixture"], row["variant"], row["mode"])
        groups.setdefault(key, []).append(row)
    counted = {(r["workload"], r["fixture"], r["variant"], r["mode"]): r for r in data["counted_rows"]}
    summary = []
    for key, rows in groups.items():
        row = dict(zip(("workload", "fixture", "variant", "mode"), key))
        row["ms_per_op"] = statistics.median(r["elapsed_ns"] / r["iterations"] / 1e6 for r in rows)
        row["process_cpu_ms_per_op"] = statistics.median(r["process_cpu_ns"] / r["iterations"] / 1e6 for r in rows)
        row["min_ms_per_op"] = min(r["elapsed_ns"] / r["iterations"] / 1e6 for r in rows)
        row["max_ms_per_op"] = max(r["elapsed_ns"] / r["iterations"] / 1e6 for r in rows)
        for field in ("max_rss_bytes", "peak_footprint_bytes", "gc_cpu_seconds", "gc_assist_seconds", "gc_cycles"):
            values = [r[field] for r in rows if field in r]
            if values:
                row[field] = statistics.median(values)
        if key[2].startswith("go"):
            row["allocated_bytes_per_op"] = statistics.median(r["allocated_bytes"] / r["iterations"] for r in rows)
            row["allocations_per_op"] = statistics.median(r["allocations"] / r["iterations"] for r in rows)
            row["gc_cpu_ms_per_op"] = statistics.median(r["gc_cpu_seconds"] / r["iterations"] * 1000 for r in rows)
            row["gc_assist_ms_per_op"] = statistics.median(r["gc_assist_seconds"] / r["iterations"] * 1000 for r in rows)
        elif key in counted:
            count = counted[key]
            row["allocated_bytes_per_op"] = count["allocated_bytes"] / count["iterations"]
            row["allocations_per_op"] = count["allocations"] / count["iterations"]
        summary.append(row)
    dump(ROOT / f"{PREFIX}-summary.json", {"rows": summary, "notes": data["notes"]})
    lines = ["# Rust / Go isolated kernel measurements", "", "Medians of fresh-process repetitions. Process peaks include setup, not only kernels.", "", "| Workload | Fixture | Variant / mode | Wall ms/op | CPU ms/op | Alloc MiB/op | Allocs/op | Peak RSS MiB |", "|---|---|---|---:|---:|---:|---:|---:|"]
    for row in summary:
        lines.append(f"| {row['workload']} | {row['fixture']} | {row['variant']} / {row['mode']} | {row['ms_per_op']:.4f} | {row['process_cpu_ms_per_op']:.4f} | {row.get('allocated_bytes_per_op',0)/2**20:.4f} | {row.get('allocations_per_op',0):.1f} | {row.get('max_rss_bytes',0)/2**20:.2f} |")
    lines += ["", "## Limits", ""] + [f"- {note}" for note in data["notes"]]
    (ROOT / f"{PREFIX}-results.md").write_text("\n".join(lines) + "\n")
    print(f"saved {PREFIX}-summary.json and {PREFIX}-results.md", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("action", choices=("build", "prepare", "validate", "bench", "summarize"))
    parser.add_argument("--no-jemalloc", action="store_true")
    parser.add_argument("--append", action="store_true")
    parser.add_argument("--session", action="append", default=[])
    parser.add_argument("--runs", type=int, default=5)
    parser.add_argument("--seconds", type=float, default=0.3)
    parser.add_argument("--only", default="")
    args = parser.parse_args()
    if args.runs < 1 or args.seconds <= 0:
        parser.error("runs and seconds must be positive")
    globals()[{"bench": "benchmark"}.get(args.action, args.action)](args)


if __name__ == "__main__":
    main()
