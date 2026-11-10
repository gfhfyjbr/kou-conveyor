#!/usr/bin/env python3
"""Compare pure durable transitions. No returned compute dispatch is executed."""
from decimal import Decimal
import json
import os
from pathlib import Path
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def operation(kind="file", state=None, status="ready", version=1):
    if state is None:
        state = {"Action": "read", "Path": "/owned-inert-fixture"} if kind == "file" else {"Value": None}
    return {"ID": "owned-operation", "Type": kind, "Version": version, "Status": status, "State": state}


def fixtures():
    for value in [None, False, 0, 42, "é\ntext", [1, None], {"items": [True, {"x": "y"}]}]:
        yield "value-spec-" + repr(value), {"Action": "value_spec", "Value": value}
        for action in ["value_decode", "value_advance"]:
            yield action + "-" + repr(value), {"Action": action, "Operation": operation("value", {"Value": value})}
    yield "opaque-number-spec", '{"Action":"value_spec","Value":1e999}'
    yield "opaque-number-advance", '{"Action":"value_advance","Operation":{"ID":"owned-operation","Type":"value","Version":1,"Status":"ready","State":{"Value":1e999}}}'
    yield "missing-value-spec", {"Action": "value_spec"}
    for state in [None, [], {}, True, 12, "text"]:
        current = operation("value")
        current["State"] = state
        yield "value-state-" + repr(state), {"Action": "value_decode", "Operation": current}
    missing = operation("value")
    del missing["State"]
    yield "missing-value-state", {"Action": "value_decode", "Operation": missing}
    for kind, version in [("shell", 1), ("value", 0), ("value", 2)]:
        yield "value-type-version-" + str((kind, version)), {"Action": "value_decode", "Operation": operation(kind, {"Value": None}, version=version)}
    for status in ["ready", "canceling", "awaiting", "completed", "failed", "canceled", "unknown"]:
        yield "value-status-" + status, {"Action": "value_advance", "Operation": operation("value", status=status)}
    yield "value-unexpected-event", {"Action": "value_advance", "Operation": operation("value"), "Event": {}}
    valid = [
        {"Action": "read", "Path": "/owned-inert-fixture"},
        {"Action": "write", "Path": "/owned-inert-fixture", "Content": "body"},
        {"Action": "edit", "Path": "/owned-inert-fixture", "OldString": "a", "NewString": "b", "ReplaceAll": True},
        {"Action": "patch", "Root": "/owned-inert-root", "Patch": "*** Begin Patch\n*** End Patch"},
        {"Action": "transcript_search", "Path": "/owned-inert-fixture", "Query": "owned query", "Limit": 3},
    ]
    for state in valid:
        yield "file-spec-" + state["Action"], {"Action": "file_spec", "File": state}
        yield "file-decode-" + state["Action"], {"Action": "file_decode", "Operation": operation(state=state)}
        for status in ["ready", "awaiting", "canceling", "completed", "failed", "canceled"]:
            yield "file-" + state["Action"] + "-" + status, {"Action": "file_advance", "Operation": operation(state=state, status=status)}
    for state in [
        {}, {"Action": "unknown"}, {"Action": "read"}, {"Action": "patch", "Patch": " \n\t"},
        {"Action": "transcript_search", "Query": "query"},
        {"Action": "transcript_search", "Path": "/owned-inert-fixture", "Query": " \t"},
        {"Action": "read", "Path": "/owned-inert-fixture", "Offset": -1},
        {"Action": "write", "Path": "/owned-inert-fixture", "Limit": -1},
        {"Action": "read", "Path": None},
    ]:
        yield "invalid-file-spec-" + repr(state), {"Action": "file_spec", "File": state}
        yield "invalid-file-decode-" + repr(state), {"Action": "file_decode", "Operation": operation(state=state)}
    for state in [None, [], True, 12, "text"]:
        current = operation()
        current["State"] = state
        yield "file-state-" + repr(state), {"Action": "file_decode", "Operation": current}
    for kind, version in [("shell", 1), ("file", 0), ("file", 2)]:
        yield "file-type-version-" + str((kind, version)), {"Action": "file_decode", "Operation": operation(kind, {"Action": "read", "Path": "/owned-inert-fixture"}, version=version)}
    for field, value in [("Result", {}), ("TerminalError", "already failed")]:
        state = {"Action": "read", "Path": "/owned-inert-fixture", field: value}
        yield "noninitial-file-" + field, {"Action": "file_advance", "Operation": operation(state=state)}
    events = [
        {"Type": "primitive.canceled"},
        {"Type": "primitive.failed"},
        {"Type": "primitive.failed", "ResultKind": "failure", "Result": {"Error": "owned failure"}},
        {"Type": "compute.completed"},
        {"Type": "compute.completed", "ResultKind": "compute", "Result": {"Text": "wrong dynamic type"}},
        {"Type": "compute.completed", "ResultKind": "file", "Result": {"Text": "owned result"}},
        {"Type": "compute.completed", "ResultKind": "file", "Result": {"Text": "é", "Files": [], "Lines": 7, "From": 2, "To": 3, "Truncated": True}},
        {"Type": "compute.completed", "ResultKind": "file", "Result": None},
        {"Type": "compute.progress"},
        {"Type": "unknown"},
    ]
    for index, event in enumerate(events):
        for source in ["owned-operation", "wrong-source"]:
            yield f"file-event-{index}-{source}", {"Action": "file_advance", "Operation": operation(status="awaiting"), "Event": {"Source": source, **event}}
    for status in ["ready", "canceling", "completed"]:
        yield "unexpected-file-event-" + status, {"Action": "file_advance", "Operation": operation(status=status), "Event": {"Source": "owned-operation", **events[5]}}
    current = operation()
    current["MaxOutputLength"] = 4321
    current["Idempotency"] = {"key": "owned fixture"}
    yield "preserve-operation-metadata", {"Action": "file_advance", "Operation": current}
    nulls = {key: None for key in ["Root", "Offset", "Limit", "Content", "OldString", "NewString", "ReplaceAll", "Patch", "Query", "Result", "TerminalError"]}
    yield "null-file-fields", {"Action": "file_decode", "Operation": operation(state={**valid[0], **nulls})}


def invoke(binary, rows):
    text = "\n".join(spec if isinstance(spec, str) else json.dumps(spec, ensure_ascii=False) for _, spec in rows) + "\n"
    completed = subprocess.run([str(binary)], input=text, text=True, capture_output=True, timeout=30, cwd=ROOT)
    if completed.returncode:
        raise RuntimeError(f"{binary.name} exited {completed.returncode}: {completed.stderr}")
    lines = completed.stdout.splitlines()
    assert len(lines) == len(rows), (binary, len(lines), len(rows))
    return [json.loads(line, parse_float=Decimal) for line in lines]


def projections_match(left, right):
    if left == right:
        return True
    # Go 1.27 encoding/json/v2/errors.go errorModalVerb deliberately chooses
    # either rendering once per process. Permit only that token in these exact
    # top-level decode diagnostics; payloads, context and every other byte stay.
    pattern = (r'(decode (?:value|file) operation "(?:\\.|[^"\\])*" state: json: )'
               r'(?:cannot|unable to)'
               r'( unmarshal JSON (?:array|string|boolean|number) into Go operation\.(?:ValueState|FileState))')
    a = re.fullmatch(pattern, left.get("Error", ""))
    b = re.fullmatch(pattern, right.get("Error", ""))
    return bool(a and b and a.groups() == b.groups()
                and {**left, "Error": right["Error"]} == right)


def comparator_self_test():
    text = 'decode value operation "owned" state: json: cannot unmarshal JSON array into Go operation.ValueState'
    left = {"Error": text, "Value": {"Error": text}, "Dispatches": [1, 2]}
    right = {**left, "Error": text.replace("json: cannot", "json: unable to")}
    assert projections_match(left, right)
    for changed in [
        {**right, "Error": right["Error"].replace('"owned"', '"other"')},
        {**right, "Error": right["Error"].replace("JSON array", "JSON number")},
        {**right, "Error": right["Error"].replace("unable to", "could not")},
        {**right, "Value": {"Error": right["Error"]}},
        {**right, "Dispatches": [2, 1]},
    ]:
        assert not projections_match(left, changed)
    assert not projections_match({"Error": "opaque cannot"}, {"Error": "opaque unable to"})


def main():
    comparator_self_test()
    directory = ROOT / "tmp/rust-operation-oracle"
    directory.mkdir(parents=True, exist_ok=True)
    go = directory / "go-operation-oracle"
    subprocess.run(["go", "build", "-o", str(go), "./cmd/kou-operation-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-runtime", "--example", "operation_oracle"], cwd=ROOT, env={**os.environ, "CARGO_INCREMENTAL": "0"}, check=True)
    rust = ROOT / "tmp/rust-rewrite-target/debug/examples/operation_oracle"
    rows = list(fixtures())
    expected, actual = invoke(go, rows), invoke(rust, rows)
    for (name, spec), left, right in zip(rows, expected, actual):
        if not projections_match(left, right):
            output = directory / "failure.json"
            output.write_text(json.dumps({"Case": name, "Spec": spec, "Go": left, "Rust": right}, ensure_ascii=False, indent=2, default=str))
            print("FAIL:", name, "\nFull synthetic diff:", output)
            raise SystemExit(1)
    print(f"PASS: {len(rows)} exact pure Go/Rust file/value transitions; dispatches never executed")


if __name__ == "__main__":
    main()
