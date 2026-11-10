#!/usr/bin/env python3
"""Exercise both writers and the real cockpit flock in private temp directories."""
import json
from pathlib import Path
import selectors
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "tmp/rust-rewrite-target"
GO = TARGET / "kou-conveyor-oracle"
RUST = TARGET / "debug/kou-session"
FIXTURE = TARGET / "debug/examples/fixture"


def run(args, success=True):
    result = subprocess.run([str(x) for x in args], cwd=ROOT, capture_output=True, timeout=60)
    if success and result.returncode:
        raise AssertionError(f"test command failed: {result.stderr.decode(errors='replace')}")
    return result


def parity(path):
    go = run([GO, "-session", path])
    rust = run([RUST, "archive", path])
    assert json.loads(go.stdout) == json.loads(rust.stdout), "writer state mismatch (bodies not logged)"


def holding(command):
    process = subprocess.Popen([str(x) for x in command], cwd=ROOT, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
    try:
        with selectors.DefaultSelector() as ready:
            ready.register(process.stdout, selectors.EVENT_READ)
            assert ready.select(timeout=10), "lock holder readiness timeout"
            assert process.stdout.readline() == b"locked\n", "lock holder failed"
        return process
    except BaseException:
        process.terminate()
        process.communicate(timeout=10)
        raise


def release(process):
    process.stdin.close()
    try:
        process.wait(timeout=10)
    finally:
        if process.poll() is None:
            process.terminate()
            process.wait(timeout=10)
    assert process.returncode == 0


def main():
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-store", "--example", "fixture"], cwd=ROOT, check=True)
    # Build the session utility separately: --example intentionally selects only examples.
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-session"], cwd=ROOT, check=True)
    subprocess.run(["go", "build", "-trimpath", "-o", str(GO), "./cmd/kou-conveyor-oracle"], cwd=ROOT, check=True)
    with tempfile.TemporaryDirectory(prefix="kou-writer-parity-") as temp:
        directory = Path(temp)
        fresh = directory / "fresh"
        run([FIXTURE, "create", fresh])
        created = fresh / "rust-session.session.jsonl"
        parity(created)
        print("PASS: Rust create -> Go read")
        run([GO, "-session", created, "-action", "append"])
        parity(created)
        print("PASS: Go append -> Rust replay")
        run([FIXTURE, "append", fresh, "rust-session"])
        parity(created)
        print("PASS: Rust append -> Go replay")
        run([FIXTURE, "fork", fresh, "rust-session", "rust-child", "rust-turn"])
        parity(fresh / "rust-child.session.jsonl")
        print("PASS: Rust fork -> Go replay with inherited ownership")
        run([FIXTURE, "checkpoint", fresh, "rust-session"])
        go_resume = json.loads(run([GO, "-session", created, "-action", "resume"]).stdout)
        rust_resume = json.loads(run([RUST, "resume", created]).stdout)
        assert go_resume == rust_resume, "Rust checkpoint -> Go resume mismatch"
        assert go_resume["Checkpoint"]["State"] == {"generation": 1}
        run([GO, "-session", created, "-action", "append"])
        run([GO, "-session", created, "-action", "checkpoint"])
        run([FIXTURE, "append", fresh, "rust-session"])
        go_resume = json.loads(run([GO, "-session", created, "-action", "resume"]).stdout)
        rust_resume = json.loads(run([RUST, "resume", created]).stdout)
        assert go_resume == rust_resume, "Go checkpoint -> Rust resume/append mismatch"
        assert rust_resume["Checkpoint"]["State"] == {"generation": 2}
        parity(created)
        print("PASS: bidirectional checkpoint envelope/head/state/tail and continuation")
        original = directory / "golden-session.session.jsonl"
        shutil.copyfile(ROOT / "harness/sessionstore/localfile/testdata/golden-session.session.jsonl", original)
        run([FIXTURE, "append", directory, "golden-session"])
        parity(original)
        print("PASS: Rust continues original Go fixture")
        process = holding([FIXTURE, "hold", fresh, "rust-session"])
        try:
            before = created.read_bytes()
            assert run([GO, "-session", created, "-action", "try-lock"], success=False).returncode != 0
            assert run([GO, "-session", created, "-action", "append"], success=False).returncode != 0
            assert created.read_bytes() == before
        finally:
            release(process)
        print("PASS: Rust owner excludes real Go cockpit lock/writer")
        process = holding([GO, "-session", created, "-action", "hold"])
        try:
            before = created.read_bytes()
            assert run([FIXTURE, "append", fresh, "rust-session"], success=False).returncode != 0
            assert created.read_bytes() == before
        finally:
            release(process)
        run([GO, "-session", created, "-action", "try-lock"])
        run([FIXTURE, "append", fresh, "rust-session"])
        parity(created)
        print("PASS: Go owner excludes Rust writer; both recover after close")


if __name__ == "__main__":
    main()
