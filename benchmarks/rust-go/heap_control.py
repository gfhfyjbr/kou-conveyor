#!/usr/bin/env python3
"""Check sensitivity to a larger retained heap; this is not a production-web model."""
import json
import statistics

import run as runner


def main():
    fixtures = json.loads(runner.MANIFEST.read_text())
    scans = max(fixtures["scans"], key=lambda row: row["bytes"])
    snapshot = max(fixtures["snapshots"], key=lambda row: row["bytes"])
    jobs = [("scan", scans, 10), ("scan-reuse", scans, 10), ("clone", snapshot, 500), ("clone-batch", snapshot, 500)]
    rows = []
    for mode, fixture, iterations in jobs:
        for repeat in range(5):
            command, env = runner.go_probe(mode, fixture["path"], iterations)
            env.update(RUSTGO_LIVE_MIB="64", GOMAXPROCS="16")
            row = runner.timed(command, env)
            assert row["ballast_bytes"] == 64 << 20
            row.update(fixture=fixture["name"], repeat=repeat)
            rows.append(row)
            print(f"{mode:11} live64  wall {row['elapsed_ns']/iterations/1e6:.4f} ms/op  CPU {row['process_cpu_ns']/iterations/1e6:.4f} ms/op", flush=True)
    summary = []
    for mode, fixture, _ in jobs:
        group = [row for row in rows if row["mode"] == mode]
        summary.append({
            "mode": mode, "fixture": fixture["name"], "ballast_bytes": 64 << 20,
            "ms_per_op": statistics.median(row["elapsed_ns"]/row["iterations"]/1e6 for row in group),
            "process_cpu_ms_per_op": statistics.median(row["process_cpu_ns"]/row["iterations"]/1e6 for row in group),
            "gc_cpu_ms_per_op": statistics.median(row["gc_cpu_seconds"]/row["iterations"]*1000 for row in group),
            "heap_before": statistics.median(row["heap_before"] for row in group),
        })
    runner.dump(runner.ROOT / "rust-go-heap-control.json", {"rows": rows, "summary": summary, "notes": [
        "64 MiB of touched pointer-free live bytes changes the GC heap goal; no changes to production processes.",
        "This does not emulate a real web heap's pointer graph and is not its measured CPU profile.",
        "Do not compare this control's RSS to Rust without adding the same unrelated 64 MiB there.",
    ]})


if __name__ == "__main__":
    main()
