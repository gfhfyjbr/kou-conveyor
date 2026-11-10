#!/usr/bin/env python3
"""Pure shell v3 transitions. No dispatch or arbitrary-PID timer is executed.

Wall-clock process-start/timer/result branches have separate native deterministic
clock tests; they are not made to look exact by stripping timestamps here.
"""
import base64
import json
import os
from pathlib import Path
import random
import subprocess

from operation_parity import comparator_self_test, projections_match

ROOT = Path(__file__).resolve().parents[1]


def state(**changes):
    return {"Input": {"Shell": "/bin/sh", "Command": "printf inert", "Directory": "/inert"}, "BaseDirectory": "/owned-inert", **changes}


def operation(phase="", status="ready", limit=29, **changes):
    return {"ID": "owned", "Type": "shell", "Version": 3, "Status": status,
            "MaxOutputLength": limit, "State": state(Phase=phase, **changes)}


def event(kind, correlation, result_kind="raw", result=None, source="owned"):
    return {"Type": kind, "Source": source, "CorrelationID": correlation, "ResultKind": result_kind, "Result": result}


def advance(current, **changes):
    return {"Action": "shell_advance", "Operation": current, **changes}


def read_events(phase, data, offset=0):
    if data:
        split = max(1, len(data) // 3)
        for start in range(0, len(data), split):
            yield event("io.read_output", phase, "read_output", {"Offset": offset + start, "Data": base64.b64encode(data[start:start+split]).decode()})


def stream_events(phase, data, limit):
    prefix = data[:4 * limit]
    yield from read_events(phase, prefix)
    yield event("io.read_completed", phase, "read_completed", {"Size": len(data)})
    if len(data) > 4 * limit:
        count = 4 * (limit - limit // 2)
        yield from read_events(phase + "_tail", data[-count:], len(data) - count)
        yield event("io.read_completed", phase + "_tail", "read_completed", {"Size": len(data)})


def fixtures():
    for limit in [-1, 0, 1, 19, 1000000, 1000001]:
        for timeout in [-1, 0, 0.25, 14400, 14400.1]:
            yield f"spec-{limit}-{timeout}", {"Action": "shell_spec", "Input": {"Shell": "/bin/sh", "Timeout": timeout}, "BaseDirectory": "/owned-inert", "MaxOutputLength": limit}
    for shell, base in [("sh", "/owned"), ("", "/owned"), ("/bin/sh", ""), ("/bin/sh", "relative")]:
        yield "spec-invalid-" + shell + base, {"Action": "shell_spec", "Input": {"Shell": shell}, "BaseDirectory": base, "MaxOutputLength": 100}
    yield "ready", advance(operation())
    for identifier in ["", ".", "..", "/absolute", "a/b", "a/", "a\\b", "中", "line\nname"]:
        current = operation(); current["ID"] = identifier
        yield "id-" + repr(identifier), advance(current)
    for base in ["/owned/./nested/../", "//owned///base//", "/../owned"]:
        current = operation(); current["State"]["BaseDirectory"] = base
        yield "clean-" + base, advance(current)
    for kind in ["file", "value", ""]:
        current = operation(); current["Type"] = kind
        yield "type-" + kind, advance(current)
    for version in [0, 1, 2, 4]:
        current = operation(); current["Version"] = version
        yield f"version-{version}", advance(current)
    for limit in [-1, 0, 1000001]:
        yield f"limit-{limit}", advance(operation(limit=limit))
    for raw in [None, [], "text", 7, True]:
        current = operation(); current["State"] = raw
        yield "decode-shape-" + repr(raw), {"Action": "shell_decode", "Operation": current}
        yield "advance-shape-" + repr(raw), advance(current)
    current = operation(); del current["State"]
    yield "missing-state", advance(current)
    for key, value in [("ProcessGroupID", -1), ("ProcessGroupID", 1), ("OutSize", -1), ("ErrSize", -1), ("Input", {"Shell": "relative"}), ("BaseDirectory", "relative"), ("InlineOut", "YQ==" * 0)]:
        yield f"validate-{key}-{value}", advance(operation(**{key: value}))
    for key in ["InlineOut", "InlineErr", "InlineOutTail", "InlineErrTail"]:
        yield "oversized-" + key, advance(operation(limit=1, **{key: base64.b64encode(b"xxxxx").decode()}))
    for text in ["YQ==", "YR==", "YQ==\r\n", None]:
        current = operation(InlineOut=text)
        yield "base64-" + repr(text), {"Action": "shell_decode", "Operation": current}
    for key in ["Input", "InlineOut", "InlineErrTail", "StartedAt", "TerminalError", "OutTruncated", "Phase", "ProcessGroupID"]:
        current = operation(); current["State"][key] = None
        yield "null-field-" + key, {"Action": "shell_decode", "Operation": current}
    for status in ["completed", "failed", "canceled", "unknown", "canceling"]:
        yield "status-" + status, advance(operation(status=status))
        yield "status-event-" + status, advance(operation(status=status), Event=event("unexpected", ""))
    yield "ready-event", advance(operation(), Event=event("unexpected", ""))
    for values in [{"Phase": "process"}, {"Result": {}}, {"TerminalError": "stale"}]:
        current = operation(); current["State"].update(values)
        yield "ready-noninitial-" + repr(values), advance(current)
    phases = ["create_directory", "create_out", "create_err", "process", "read_out", "read_out_tail", "read_err", "read_err_tail", "bad", ""]
    for phase in phases:
        current = operation(phase, "awaiting", OutSize=1000, ErrSize=1000)
        yield "resume-" + phase, advance(current)
        for evt in [event("primitive.failed", "bad", "failure", {"Error": "owned failure"}),
                    event("primitive.failed", "bad"), event("primitive.canceled", "bad"),
                    event("unexpected", "bad"), event("unexpected", phase, source="foreign"),
                    event("unexpected", phase), event("timer.fired", "timeout"), event("timer.fired", "kill")]:
            yield f"phase-event-{phase}-{evt}", advance(current, Event=evt)
    for pid in [0, 2, 4321]:
        yield f"process-recovery-{pid}", advance(operation("process", "awaiting", ProcessGroupID=pid))
    for phase, expected in [("create_directory", 2), ("create_out", 1), ("create_err", 1)]:
        for kind in [0, 1, 2, 3]:
            yield f"create-{phase}-{kind}", advance(operation(phase, "awaiting"), Event=event("io.create_completed", phase, "created", {"Kind": kind}))
        yield "create-raw-" + phase, advance(operation(phase, "awaiting"), Event=event("io.create_completed", phase))
    for kind in ["process.output", "process.stream_failed", "process.input_written", "unknown"]:
        yield "process-event-" + kind, advance(operation("process", "awaiting"), Event=event(kind, "process"))
    for code, signal in [(0, 0), (7, 0), (-1, 15), (-1, 9)]:
        yield f"exit-{code}-{signal}", advance(operation("process", "awaiting", ProcessGroupID=4321), Event=event("process.exited", "process", "exited", {"ExitCode": code, "Signal": signal}))
    for pid in [-1, 0, 1]:
        yield f"invalid-start-{pid}", advance(operation("process", "awaiting"), Event=event("process.started", "process", "started", {"PID": pid}))
    for kind in ["process.started", "process.exited"]:
        yield "invalid-result-" + kind, advance(operation("process", "awaiting"), Event=event(kind, "process"))
    for phase in ["read_out", "read_err", "read_out_tail", "read_err_tail"]:
        current = operation(phase, "awaiting", limit=3, OutSize=100, ErrSize=100)
        offset = 92 if phase.endswith("tail") else 0
        for evt in [event("io.read_output", phase), event("io.read_completed", phase),
                    event("io.read_output", phase, "read_output", {"Offset": offset+1, "Data": "YQ=="}),
                    event("io.read_output", phase, "read_output", {"Offset": offset, "Data": base64.b64encode(b"x" * 100).decode()}),
                    event("io.read_completed", phase, "read_completed", {"Size": -1}),
                    event("io.read_completed", phase, "read_completed", {"Size": 1})]:
            yield f"bad-read-{phase}-{evt}", advance(current, Event=evt)
    for limit in [1, 2, 3, 10, 19]:
        for reason in ["short", "long error " * 100, "中é🙂" * 20]:
            yield f"bound-error-{limit}-{reason[:8]}", advance(operation("process", "awaiting", limit=limit), Event=event("primitive.failed", "process", "failure", {"Error": reason}))
    rng = random.Random(412)
    bodies = [b"", b"abc", "é中🙂suffix".encode(), bytes(range(256)), b"\xf0\x9f", b"\xff\xfeA\x80\x80", b"a" * 91]
    bodies += [rng.randbytes(rng.randrange(0, 200)) for _ in range(80)]
    for i, stdout in enumerate(bodies):
        limit = [1, 2, 3, 10, 19][i % 5]
        stderr = bodies[(i + 3) % len(bodies)]
        events = list(stream_events("read_out", stdout, limit)) + list(stream_events("read_err", stderr, limit))
        yield f"streams-{i}", advance(operation("read_out", "awaiting", limit=limit, PendingExitCode=7), Events=events)
    yield "missing-exit", advance(operation("read_err", "awaiting"), Event=event("io.read_completed", "read_err", "read_completed", {"Size": 0}))
    current = operation("read_out_tail", "awaiting", limit=3, OutSize=100)
    yield "tail-size-changed", advance(current, Events=list(read_events("read_out_tail", b"x" * 8, 92)) + [event("io.read_completed", "read_out_tail", "read_completed", {"Size": 101})])


def invoke(binary, rows):
    payload = "".join(json.dumps(spec, ensure_ascii=False) + "\n" for _, spec in rows)
    completed = subprocess.run([str(binary)], input=payload, text=True, capture_output=True, timeout=30, cwd=ROOT)
    if completed.returncode:
        raise RuntimeError(f"{binary.name} exited {completed.returncode}: {completed.stderr}")
    lines = completed.stdout.splitlines()
    assert len(lines) == len(rows), (binary, len(lines), len(rows))
    return [json.loads(line) for line in lines]


def main():
    comparator_self_test()
    directory = ROOT / "tmp/rust-shell-oracle"
    directory.mkdir(parents=True, exist_ok=True)
    go = directory / "go-shell-oracle"
    subprocess.run(["go", "build", "-o", str(go), "./cmd/kou-shell-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-runtime", "--example", "shell_oracle"], cwd=ROOT,
                   env={**os.environ, "CARGO_INCREMENTAL": "0"}, check=True)
    rust = ROOT / "tmp/rust-rewrite-target/debug/examples/shell_oracle"
    rows = list(fixtures())
    left, right = invoke(go, rows), invoke(rust, rows)
    for (name, spec), expected, actual in zip(rows, left, right):
        if not projections_match(expected, actual):
            path = directory / "failure.json"
            path.write_text(json.dumps({"Case": name, "Spec": spec, "Go": expected, "Rust": actual}, ensure_ascii=False, indent=2))
            raise SystemExit(f"FAIL: {name}; exact synthetic diff: {path}")
    print(f"PASS: {len(rows)} Go/Rust shell v3 transitions; no effects, no timestamp stripping, no unowned PID signals")


if __name__ == "__main__":
    main()
