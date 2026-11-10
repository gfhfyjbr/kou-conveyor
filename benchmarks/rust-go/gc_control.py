#!/usr/bin/env python3
"""Bounded diagnostics: separate Go GC and parallelism from the port's other changes."""
import json
import statistics

import run as runner


def main():
    fixtures = json.loads(runner.MANIFEST.read_text())
    jobs = [
        ("scan", max(fixtures["scans"], key=lambda row: row["bytes"])),
        ("clone", max(fixtures["snapshots"], key=lambda row: row["bytes"])),
    ]
    rows = []
    for mode, fixture in jobs:
        for variant, gc in (("go50-p1", "50"), ("go-off-p1", "off")):
            # GC-off allocations are capped to about 200 MiB per measured loop.
            iterations = (1 if mode == "scan" else 32) if gc == "off" else (10 if mode == "scan" else 500)
            for repeat in range(5):
                command, env = runner.go_probe(mode, fixture["path"], iterations, gc=gc)
                env["GOMAXPROCS"] = "1"
                row = runner.timed(command, env)
                row.update(variant=variant, fixture=fixture["name"], repeat=repeat)
                rows.append(row)
                print(f"{mode:5} {variant:10} {row['elapsed_ns']/iterations/1e6:.4f} ms/op, CPU {row['process_cpu_ns']/iterations/1e6:.4f} ms/op", flush=True)
    summary = []
    for mode, fixture in jobs:
        for variant in ("go50-p1", "go-off-p1"):
            group = [row for row in rows if row["mode"] == mode and row["variant"] == variant]
            summary.append({
                "mode": mode, "fixture": fixture["name"], "variant": variant,
                "ms_per_op": statistics.median(row["elapsed_ns"]/row["iterations"]/1e6 for row in group),
                "process_cpu_ms_per_op": statistics.median(row["process_cpu_ns"]/row["iterations"]/1e6 for row in group),
                "gc_cpu_ms_per_op": statistics.median(row["gc_cpu_seconds"]/row["iterations"]*1000 for row in group),
                "gc_cycles": statistics.median(row["gc_cycles"] for row in group),
                "allocated_bytes_per_op": statistics.median(row["allocated_bytes"]/row["iterations"] for row in group),
                "max_rss_bytes": statistics.median(row["max_rss_bytes"] for row in group),
            })
    runner.dump(runner.ROOT / "rust-go-gc-control.json", {
        "rows": rows, "summary": summary,
        "notes": [
            "These are diagnostics, not recommended deployment settings.",
            "GOMAXPROCS=1 is forced for both controls; the main matrix keeps the environment default.",
            "GC-off loops are short and bounded. No production process changes are made.",
            "Single-operation GC-off scan has lower statistical power than the longer main loops.",
            "Whole-process peaks include the pre-loop expected-result scan and source parsing; GC-off peaks are not isolated loop peaks.",
        ],
    })


if __name__ == "__main__":
    main()
