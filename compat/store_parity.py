#!/usr/bin/env python3
"""Compare Rust's lazy archive with the Go store without printing user bodies."""
import argparse
import json
import os
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "tmp/rust-rewrite-target"


def run(command):
    return subprocess.run(command, cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE)


def compare(path, rust, go):
    # Both implementations must read the same committed snapshot, not two
    # different moments of an active runner's file.
    before = path.stat()
    with tempfile.TemporaryDirectory(prefix="kou-parity-snapshot-") as temp:
        snapshot = Path(temp) / path.name
        shutil.copyfile(path, snapshot)
        os.chmod(snapshot, 0o600)
        after = path.stat()
        fields = ("st_dev", "st_ino", "st_size", "st_mtime_ns", "st_ctime_ns")
        if any(getattr(before, key) != getattr(after, key) for key in fields):
            return "source changed during snapshot; retry on an inactive log"
        left = run([str(go), "-session", str(snapshot)])
        right = run([str(rust), "archive", str(snapshot)])
    if bool(left.returncode) != bool(right.returncode):
        return "acceptance mismatch"
    if left.returncode:
        return None
    if json.loads(left.stdout) != json.loads(right.stdout):
        return "decoded state mismatch"
    return None


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("sessions", nargs="*", type=Path)
    parser.add_argument("--skip-build", action="store_true")
    args = parser.parse_args()
    TARGET.mkdir(parents=True, exist_ok=True)
    go = TARGET / "kou-conveyor-oracle"
    rust = TARGET / "debug/kou-session"
    if not args.skip_build:
        subprocess.run(["cargo", "build", "--locked", "-p", "kou-session"], cwd=ROOT, check=True)
        subprocess.run(["go", "build", "-trimpath", "-o", str(go), "./cmd/kou-conveyor-oracle"], cwd=ROOT, check=True)
    paths = sorted((ROOT / "harness/sessionstore/localfile/testdata").glob("*.session.jsonl"))
    paths += [path.resolve() for path in args.sessions]
    failures = []
    for index, path in enumerate(paths):
        error = compare(path, rust, go)
        print(f"case {index + 1}: {error or 'PASS'}")
        if error:
            failures.append((index + 1, error))
    with tempfile.TemporaryDirectory(prefix="kou-parity-") as temp:
        source = ROOT / "harness/sessionstore/localfile/testdata/golden-session.session.jsonl"
        original = source.read_bytes()
        lines = original.splitlines(keepends=True)
        cases = {
            "torn": original + b'{"type":',
            "crlf": original.replace(b"\n", b"\r\n"),
            "bad-tail": original + b'{bad}\n',
            "empty": b"",
            "no-newline": lines[0].rstrip(b"\n"),
            "blank": original + b"\n",
            "duplicate": original.replace(b'"Version":2', b'"Version":2,"Version":2'),
            "unknown-duplicate": original.replace(b'"Version":2', b'"Version":2,"unused":{"a":1,"a":2}'),
            "sequence-gap": original.replace(b'"Sequence":2', b'"Sequence":200'),
            "missing-operation": original.replace(b'"operation-1"', b'"operation-missing"', 1),
            "wrong-operation-type": original.replace(b'"Type":"test","Version":1,"Status":"awaiting"', b'"Type":"other","Version":1,"Status":"awaiting"'),
            "wrong-time": original.replace(b'"2026-08-27T10:01:00Z"', b'"0001-01-01T00:00:00Z"', 1),
            "offset-time": original.replace(b'2026-08-27T10:00:00Z', b'2026-08-27T12:00:00+02:00'),
            "fraction-time": original.replace(b'2026-08-27T10:00:00Z', b'2026-08-27T10:00:00.123400Z'),
            "opaque-large-number": original.replace(b'"Version":2', b'"Version":2,"unknown":1e999'),
            "deep-opaque": original.replace(b'"Version":2', b'"Version":2,"unknown":' + b'[' * 1000 + b'0' + b']' * 1000),
            "too-deep": original.replace(b'"Version":2', b'"Version":2,"unknown":' + b'[' * 10001 + b'0' + b']' * 10001),
            "invalid-surrogate": original.replace(b'"Version":2', b'"Version":2,"unknown":"\\ud800"'),
        }
        for name, data in cases.items():
            directory = Path(temp) / name
            directory.mkdir()
            path = directory / "golden-session.session.jsonl"
            path.write_bytes(data)
            os.chmod(path, 0o600)
            error = compare(path, rust, go)
            print(f"{name}: {error or 'PASS'}")
            if error:
                failures.append((name, error))
    if failures:
        raise SystemExit(f"{len(failures)} parity checks failed; bodies were not logged")
    print(f"PASS: {len(paths) + len(cases)} store parity checks")


if __name__ == "__main__":
    main()
