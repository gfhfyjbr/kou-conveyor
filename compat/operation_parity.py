#!/usr/bin/env python3
"""Compare pure durable transitions. No returned compute dispatch is executed."""
from decimal import Decimal
import base64
import random
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
    for index, result in enumerate([[], ["positional"], ["positional", [], 1, 2, 3, False], "text", 42, True, None, {"Text": None}]):
        for action in ["file_decode", "file_advance"]:
            yield f"file-result-shape-{index}-{action}", {"Action": action, "Operation": operation(state={**valid[0], "Result": result})}


def skill_operation(status="awaiting", content=b"", **fields):
    return {"ID": "owned", "Type": "skill_use", "Version": 1, "Status": status,
            "State": {"Path": "/inert/raw/../SKILL.md", "Content": base64.b64encode(content).decode(), **fields}}


def skill_event(kind, result_kind="raw", result=None, source="owned", correlation="skill-use-read"):
    value = {"Type": kind, "Source": source, "CorrelationID": correlation, "ResultKind": result_kind}
    if result is not None:
        value["Result"] = result
    return value


def skill_fixtures():
    for path in ["", " \n\t", "raw/../SKILL.md", " /inert//SKILL.md ", "\0", "界/SKILL.md"]:
        yield "skill-spec-" + repr(path), {"Action": "skill_spec", "Path": path}
    for raw in [None, [], "text", 42, True, {}, {"Path": ""}, {"Path": None}, {"Path": "\n"}]:
        for action in ["skill_decode", "skill_advance"]:
            current = skill_operation(); current["State"] = raw
            yield "skill-shape-" + action + repr(raw), {"Action": action, "Operation": current}
    current = skill_operation(); del current["State"]
    yield "skill-missing-state", {"Action": "skill_decode", "Operation": current}
    for changes in [{"Type": "file"}, {"Type": ""}, {"Version": 0}, {"Version": 2}]:
        yield "skill-header-" + repr(changes), {"Action": "skill_advance", "Operation": {**skill_operation(), **changes}}
    for field, value in [("Path", 42), ("Path", []), ("Path", {}), ("TerminalError", False),
                         ("Content", []), ("Content", [1, 2]), ("Content", {}), ("Content", True), ("Content", 42),
                         ("Content", None), ("Content", ""), ("Content", "YR=="), ("Content", "YQ==\r\n"), ("TerminalError", None)]:
        yield "skill-field-" + field + repr(value), {"Action": "skill_decode", "Operation": skill_operation(**{field: value})}
    for fields in [{"Path": 42, "Content": []}, {"Content": [], "Path": 42},
                   {"TerminalError": [], "Content": []}, {"Content": [], "TerminalError": []}]:
        current = skill_operation(); current["State"] = fields
        yield "skill-error-order-" + repr(fields), {"Action": "skill_decode", "Operation": current}
    for status in ["ready", "awaiting", "canceling", "completed", "failed", "canceled", "invalid"]:
        for body in [b"", b"saved\xff\x00"]:
            for error in ["", "stale"]:
                current = skill_operation(status, body, TerminalError=error)
                yield "skill-resume-" + repr((status, body, error)), {"Action": "skill_advance", "Operation": current}
                yield "skill-event-status-" + repr((status, body, error)), {"Action": "skill_advance", "Operation": current, "Event": skill_event("unknown")}
    for evt in [skill_event("io.read_output"), skill_event("io.read_output", "raw", {}),
                skill_event("io.read_completed"), skill_event("io.read_completed", "read_output", {"Offset": 0, "Data": ""}),
                skill_event("io.read_output", "read_completed", {"Size": 0}),
                skill_event("io.read_output", "compute", {"opaque": "payload"}),
                skill_event("primitive.failed"), skill_event("primitive.failed", "compute", {}),
                skill_event("primitive.failed", "failure", {"Error": ""}),
                skill_event("primitive.failed", "failure", {"Error": "owned failure\n"}),
                skill_event("primitive.canceled", "raw", 42), skill_event("unknown"),
                skill_event("primitive.failed", "failure", {"Error": "foreign"}, source="other"),
                skill_event("io.read_completed", "read_completed", {"Size": 0}, correlation="wrong"),
                skill_event("primitive.canceled", correlation="wrong")]:
        yield "skill-event-" + repr(evt), {"Action": "skill_advance", "Operation": skill_operation(), "Event": evt}
    for body in [b"", b"saved", b"\xff\x00\xe4\xb8\xad"]:
        current = skill_operation(content=body)
        for offset in [-1, 0, len(body), len(body) + 1]:
            for suffix in [b"", b"new", b"\x80\x00"]:
                evt = skill_event("io.read_output", "read_output", {"Offset": offset, "Data": base64.b64encode(suffix).decode()})
                yield "skill-offset-" + repr((body, offset, suffix)), {"Action": "skill_advance", "Operation": current, "Event": evt}
        for size in [-1, 0, len(body), len(body) + 1, 9223372036854775807]:
            yield "skill-completed-" + repr((body, size)), {"Action": "skill_advance", "Operation": current, "Event": skill_event("io.read_completed", "read_completed", {"Size": size})}
    current = skill_operation("ready", b"old", TerminalError="old")
    current["MaxOutputLength"] = 1
    current["Idempotency"] = {"opaque": "preserved"}
    yield "skill-metadata", {"Action": "skill_advance", "Operation": current}
    yield "skill-error-not-clipped", {"Action": "skill_advance", "Operation": {**skill_operation(), "MaxOutputLength": 1},
                                      "Event": skill_event("primitive.failed", "failure", {"Error": "界" * 40001})}
    rng = random.Random(20261113)
    for index in range(80):
        prefix, suffix = rng.randbytes(rng.randrange(0, 200)), rng.randbytes(rng.randrange(0, 200))
        current = skill_operation(content=prefix)
        yield f"skill-random-decode-{index}", {"Action": "skill_decode", "Operation": current}
        yield f"skill-random-resume-{index}", {"Action": "skill_advance", "Operation": current}
        yield f"skill-random-append-{index}", {"Action": "skill_advance", "Operation": current,
                "Event": skill_event("io.read_output", "read_output", {"Offset": len(prefix), "Data": base64.b64encode(suffix).decode()})}
        yield f"skill-random-complete-{index}", {"Action": "skill_advance", "Operation": current,
                "Event": skill_event("io.read_completed", "read_completed", {"Size": len(prefix)})}


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
    pattern = (r'(decode (?:value|file|shell|skill-use) operation "(?:\\.|[^"\\])*" state: json: )'
               r'(?:cannot|unable to)'
               r'( unmarshal JSON (?:(?:array|string|boolean|number) into Go operation\.(?:ValueState|FileState|ShellState|SkillUseState|FileResult within "/Result")'
               r'|string into Go \[\]uint8 within "/(?:Inline(?:Out|Err)(?:Tail)?|Content)": illegal character .+ at offset [0-9]+'
               r'|(?:array|object|boolean|number) into Go (?:\[\]uint8 within "/Content"|string within "/(?:Path|TerminalError)")))')
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
    text = ('decode shell operation "owned" state: json: cannot unmarshal JSON string into Go []uint8 '
            'within "/InlineOut": illegal character ' + repr('\r') + ' at offset 4')
    left = {"Error": text, "State": None}
    right = {**left, "Error": text.replace("json: cannot", "json: unable to")}
    assert projections_match(left, right)
    assert not projections_match(left, {**right, "Error": right["Error"].replace("offset 4", "offset 5")})
    assert not projections_match(left, {**right, "Error": right["Error"].replace("/InlineOut", "/InlineErr")})
    assert not projections_match({"Error": "opaque cannot"}, {"Error": "opaque unable to"})
    text = 'decode file operation "owned" state: json: cannot unmarshal JSON array into Go operation.FileResult within "/Result"'
    left = {"Error": text, "State": None}
    right = {**left, "Error": text.replace("json: cannot", "json: unable to")}
    assert projections_match(left, right)
    assert not projections_match(left, {**right, "Error": right["Error"].replace("/Result", "/Other")})
    text = 'decode skill-use operation "owned" state: json: cannot unmarshal JSON array into Go []uint8 within "/Content"'
    left = {"Error": text, "Dispatches": [], "Value": {"Content": "opaque"}}
    right = {**left, "Error": text.replace("json: cannot", "json: unable to")}
    assert projections_match(left, right)
    for before, after in [("/Content", "/Path"), ("JSON array", "JSON object"), ('"owned"', '"other"')]:
        assert not projections_match(left, {**right, "Error": right["Error"].replace(before, after)})
    assert not projections_match(left, {**right, "Value": {"Content": right["Error"]}})


def main():
    comparator_self_test()
    directory = ROOT / "tmp/rust-operation-oracle"
    directory.mkdir(parents=True, exist_ok=True)
    go = directory / "go-operation-oracle"
    subprocess.run(["go", "build", "-o", str(go), "./cmd/kou-operation-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-runtime", "--example", "operation_oracle"], cwd=ROOT, env={**os.environ, "CARGO_INCREMENTAL": "0"}, check=True)
    rust = ROOT / "tmp/rust-rewrite-target/debug/examples/operation_oracle"
    file_value, skills = list(fixtures()), list(skill_fixtures())
    rows = file_value + skills
    expected, actual = invoke(go, rows), invoke(rust, rows)
    for (name, spec), left, right in zip(rows, expected, actual):
        if not projections_match(left, right):
            output = directory / "failure.json"
            output.write_text(json.dumps({"Case": name, "Spec": spec, "Go": left, "Rust": right}, ensure_ascii=False, indent=2, default=str))
            print("FAIL:", name, "\nFull synthetic diff:", output)
            raise SystemExit(1)
    print(f"PASS: {len(rows)} exact pure Go/Rust transitions ({len(file_value)} file/value, {len(skills)} skill-use); dispatches never executed")


if __name__ == "__main__":
    main()
