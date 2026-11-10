#!/usr/bin/env python3
"""Measure only store recovery in fresh processes; never executes runner/tools."""
import argparse
import json
import os
from pathlib import Path
import random
import re
import shutil
import statistics
import subprocess
import tempfile
import time

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "tmp/rust-rewrite-target"


def timed(command):
    started = time.monotonic_ns()
    result = subprocess.run(["/usr/bin/time", "-l", *map(str, command)], cwd=ROOT,
                            stdout=subprocess.DEVNULL, stderr=subprocess.PIPE, check=True)
    timing = result.stderr.decode()
    def integer(pattern):
        match = re.search(pattern, timing)
        if not match:
            raise RuntimeError("missing time measurement")
        return int(match[1])
    cpu = re.search(r"([\d.]+)\s+real\s+([\d.]+)\s+user\s+([\d.]+)\s+sys", timing)
    if not cpu:
        raise RuntimeError("missing CPU measurement")
    return {"wall_ms": (time.monotonic_ns() - started) / 1e6,
            "cpu_ms": (float(cpu[2]) + float(cpu[3])) * 1000,
            "peak_rss_bytes": integer(r"(\d+)\s+maximum resident set size"),
            "peak_footprint_bytes": integer(r"(\d+)\s+peak memory footprint")}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sessions", nargs="*", type=Path)
    parser.add_argument("--synthetic-checkpoint-mib", type=int, default=0)
    parser.add_argument("--repetitions", type=int, default=5)
    parser.add_argument("--output", type=Path, default=ROOT / "rust-rewrite-recovery-bench.json")
    args = parser.parse_args()
    if os.uname().sysname != "Darwin" or not 1 <= args.repetitions <= 20:
        parser.error("requires Darwin time -l and 1..20 repetitions")
    if not 0 <= args.synthetic_checkpoint_mib <= 128 or not (args.sessions or args.synthetic_checkpoint_mib):
        parser.error("provide session files or --synthetic-checkpoint-mib 1..128")
    subprocess.run(["cargo", "build", "--locked", "--release", "-p", "kou-session"], cwd=ROOT, check=True)
    subprocess.run(["go", "build", "-o", TARGET / "kou-conveyor-oracle", "./cmd/kou-conveyor-oracle"], cwd=ROOT, check=True)
    rows = []
    with tempfile.TemporaryDirectory(prefix="kou-recovery-bench-") as directory:
        paths = list(args.sessions)
        if args.synthetic_checkpoint_mib:
            subprocess.run(["cargo", "build", "--locked", "--release", "-p", "kou-store", "--example", "fixture"], cwd=ROOT, check=True)
            generated = Path(directory) / "synthetic-input"
            subprocess.run([TARGET / "release/examples/fixture", "large-checkpoint", generated, str(args.synthetic_checkpoint_mib * 1024 * 1024)], cwd=ROOT, check=True)
            paths.append(generated / "rust-session.session.jsonl")
        for case, source in enumerate(paths, 1):
            before = source.stat()
            snapshot = Path(directory) / source.name
            shutil.copyfile(source, snapshot)
            os.chmod(snapshot, 0o600)
            after = source.stat()
            assert all(getattr(before, key) == getattr(after, key) for key in ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")), "source changed during copy"
            commands = {
                "go-resume": [TARGET / "kou-conveyor-oracle", "-session", snapshot, "-action", "resume"],
                "rust-resume": [TARGET / "release/kou-session", "resume", snapshot],
                "rust-full-replay": [TARGET / "release/kou-session", "resume-full", snapshot],
            }
            # Warm every implementation before interleaved fresh-process trials.
            for command in commands.values():
                subprocess.run(list(map(str, command)), cwd=ROOT, stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL, check=True)
            trials = list(commands) * args.repetitions
            random.Random(case).shuffle(trials)
            samples = {key: [] for key in commands}
            for key in trials:
                samples[key].append(timed(commands[key]))
            summary = {key: {metric: statistics.median(sample[metric] for sample in values) for metric in values[0]} for key, values in samples.items()}
            candidates = 0
            carry = b""
            marker = b'\n{"type":"checkpoint","data":'
            with snapshot.open("rb") as stream:
                while chunk := stream.read(64 * 1024):
                    joined = carry + chunk
                    candidates += joined.count(marker)
                    carry = joined[-(len(marker) - 1):]
            rows.append({"case": case, "source_bytes": before.st_size, "checkpoint_candidates": candidates, "samples": samples, "median": summary})
            print(json.dumps({"case": case, "source_bytes": before.st_size, "median": summary}), flush=True)
    args.output.write_text(json.dumps({"scope": "fresh-process store resume plus JSON serialization, warm file cache; development Go oracle vs Rust session CLI (startup/linking not normalized); not runner/server/browser E2E; read-only private copies; system allocator", "repetitions": args.repetitions, "results": rows}, indent=2) + "\n")


if __name__ == "__main__":
    main()
