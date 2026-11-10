#!/usr/bin/env python3
"""Compare direct-file capture behavior, not the full primitive control protocol.

Only these trusted fixture scripts run. Oracles own private disposable directories;
no provider calls, production credentials, session writes or arbitrary PID signals.
"""
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def fixtures():
    yield "empty", {"Code": ":"}, b"", b"", 0, 0
    yield "streams-and-status", {"Code": "printf out; printf err >&2; exit 7"}, b"out", b"err", 7, 0
    yield "regular-descriptors", {"Code": "test -f /dev/stdout && test -f /dev/stderr || exit 91; printf regular"}, b"regular", b"", 0, 0
    yield "binary-output", {"Code": "printf '\\000\\377\\376a'; printf 'b\\200\\000' >&2"}, b"\x00\xff\xfea", b"b\x80\x00", 0, 0
    yield "unicode", {"Code": "printf 'начало中конец'; printf 'é\\n' >&2"}, "начало中конец".encode(), "é\n".encode(), 0, 0
    yield "full-spool", {"Code": "i=0; while [ \"$i\" -lt 20000 ]; do printf 0123456789; i=$((i+1)); done; printf end >&2"}, b"0123456789" * 20000, b"end", 0, 0
    yield "stdin", {"Code": "/bin/cat", "Input": "owned input\x00é\n"}, "owned input\x00é\n".encode(), b"", 0, 0
    yield "large-stdin", {"Code": "/bin/cat", "Input": "abc\n" * 65536}, b"abc\n" * 65536, b"", 0, 0
    yield "empty-stdin", {"Code": "/bin/cat; printf eof", "Input": ""}, b"eof", b"", 0, 0
    yield "default-stdin-eof", {"Code": "/bin/cat; printf eof"}, b"eof", b"", 0, 0
    yield "environment", {"Code": "printf '%s:%s' \"$ONLY\" \"${HOME-unset}\"", "Environment": {"ONLY": "owned"}}, b"owned:unset", b"", 0, 0
    yield "empty-environment", {"Code": "printf '%s' \"${HOME-unset}\"", "Environment": {}}, b"unset", b"", 0, 0
    yield "inherited-environment", {"Code": "printf '%s' \"$KOU_CAPTURE_ORACLE\""}, b"synthetic-parent", b"", 0, 0
    yield "working-directory", {"Code": "test \"$(/bin/pwd -P)\" = \"$1\" && test -f owned-marker && printf directory"}, b"directory", b"", 0, 0
    yield "term-status", {"Code": "kill -TERM $$"}, b"", b"", -1, 15
    yield "kill-status", {"Code": "kill -KILL $$"}, b"", b"", -1, 9
    yield "leader-exit-descendant", {"Code": "/bin/sleep 30 & printf leader; exit 3"}, b"leader", b"", 3, 0
    yield "canceled-before-start", {"Code": "printf must-not-run", "CancelBefore": True}, b"old capture", b"old capture", None, None
    yield "cancellation-final-output", {"Code": "trap 'printf final; printf term >&2; exit 0' TERM; printf ready; while :; do :; done", "CancelOnReady": True}, b"readyfinal", b"term", None, None


def invoke(binary, spec):
    completed = subprocess.run([str(binary)], input=json.dumps(spec), text=True, capture_output=True,
                               timeout=30, cwd=ROOT, env={**os.environ, "KOU_CAPTURE_ORACLE": "synthetic-parent"})
    if completed.returncode:
        raise RuntimeError(f"{binary.name} exited {completed.returncode}: {completed.stderr}")
    return json.loads(completed.stdout)


def main():
    directory = ROOT / "tmp/rust-capture-oracle"
    directory.mkdir(parents=True, exist_ok=True)
    go = directory / "go-capture-oracle"
    subprocess.run(["go", "build", "-o", str(go), "./cmd/kou-capture-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-tools", "--example", "capture_oracle"],
                   cwd=ROOT, env={**os.environ, "CARGO_INCREMENTAL": "0"}, check=True)
    rust = ROOT / "tmp/rust-rewrite-target/debug/examples/capture_oracle"
    cases = list(fixtures())
    for name, spec, stdout, stderr, exit_code, signal in cases:
        expected = {
            "Started": not spec.get("CancelBefore", False), "Exited": exit_code is not None,
            "Canceled": exit_code is None, "ExitCode": exit_code, "Signal": signal,
            "StdoutHex": stdout.hex(), "StderrHex": stderr.hex(), "OutMode": 0o600,
            "ErrMode": 0o600, "GroupGone": True, "Error": "",
        }
        left, right = invoke(go, spec), invoke(rust, spec)
        if left != expected or right != expected:
            output = directory / "failure.json"
            output.write_text(json.dumps({"Case": name, "Spec": spec, "Expected": expected, "Go": left, "Rust": right}, ensure_ascii=False, indent=2))
            raise SystemExit(f"FAIL: {name}; exact synthetic projection: {output}")
    print(f"PASS: {len(cases)} owned Go/Rust direct-file capture cases; exact bytes/modes/status and cleanup, no providers")


if __name__ == "__main__":
    main()
