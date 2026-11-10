#!/usr/bin/env python3
"""Pure model-call translation; submitted specs are recorded and never executed."""
import copy
import json
import os
from pathlib import Path
import random
import re
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def translate(arguments, config=None):
    return {
        "Action": "translate", "Name": "Bash",
        "Bash": config or {"Shell": "/bin/sh", "Directory": "/inert/work", "BaseDirectory": "/inert/captures", "DefaultTimeout": 600},
        "Call": {"CallID": "call", "Name": "Bash", "Arguments": arguments if isinstance(arguments, str) else json.dumps(arguments, ensure_ascii=False)},
    }


def operation(status="completed", state=None, **header):
    return {"ID": "owned", "MaxOutputLength": 40000, "Type": "shell", "Version": 3, "Status": status,
            "State": state if state is not None else {"Result": {}}, **header}


def result(operations=(), status=None, action="result"):
    return {"Action": action, "Name": "Bash", "Call": {"CallID": "call"}, "Status": status or {}, "Operations": list(operations)}


def cases():
    rows = []
    for raw in ["", "{", " \n", "{\"command\":\"x\"", "null", "[]", "true", "42", '"text"', '{} x',
                '{"command":"a","command":"b"}', '{"a":{"b":1,"b":2}}',
                '{"a":{"b":1,"b":2},"a":{}}', '{"a/b":{"~key":1,"~key":2}}']:
        rows.append(translate(raw))
    for command in [None, "", " ", "\t\n", "  printf test\n", "界", "a\0b", "界\0x", 42, [], {}, False]:
        rows.append(translate({"command": command}))
    rows += [translate({}), translate({"Command": "wrong-case"})]
    values = [None, "", "  ", 0, "0", -0.0, 0.1250000009, "600", "2.5e1", "+1.", ".5", "1_000",
              "0x1p2", "0X_1.fp+2", "0x1p1_0", "0x1p-1074", "0x1p-1075", "0x1.0000000000001p-1075",
              "0x1.00000000000008p0", "0x1.00000000000008001p0", "0x0p9999999999999999999999",
              -1, "-1", 14400, 14400.0001, "NaN", "Inf", "-Infinity", "soon", "0x1", "0x1.2", "0x1_p2",
              "0x1p_2", "1e_1", "1__2", "_1", "1_", True, [], {}]
    for value in values:
        rows.append(translate({"command": "true", "timeout": value}))
    for value in [None, 0, -1, 1.5, 1, 40000, "100", "1e3", "0x1p4", "", "NaN", True, [], {}, 1_000_001, 1e100, "9223372036854775808"]:
        rows.append(translate({"command": "true", "max_output_length": value}))
    for raw in ['{"command":"true","timeout":1e999}', '{"command":"true","max_output_length":1e999}']:
        rows.append(translate(raw))
    for limit in [1, 2, 3, 7, 20, 40]:
        for args in [{}, {"command": None}, {"command": "界\0x"}, {"command": "true", "timeout": -1}]:
            rows.append(translate({**args, "max_output_length": limit}))
    for limit in [1, 2, 3, 7, 20, 40]:
        for command in [42, [], {}, False]:
            rows.append(translate({"command": command, "max_output_length": limit}))
        for timeout in ["", [], False]:
            rows.append(translate({"command": "true", "timeout": timeout, "max_output_length": limit}))
    # Validation order: output limit, timeout, command, then operation construction.
    rows += [translate({"command": None, "timeout": -1}), translate({"command": None, "timeout": [], "max_output_length": 0})]
    for config in [
        {"Shell": "relative", "BaseDirectory": "/inert"},
        {"Shell": "/bin/sh", "BaseDirectory": "relative"},
        {"Shell": "/bin/sh", "BaseDirectory": "/inert", "DefaultTimeout": -1},
        {"Shell": "/bin/sh", "BaseDirectory": "/inert", "DefaultTimeout": 14401},
        {"Shell": "/bin/sh", "BaseDirectory": "/inert"},
    ]:
        rows.append(translate({"command": "true"}, config))
    rows += [result(), result([operation(), operation()]), result(status={"Error": "validation"}),
             result([operation()], {"Error": "validation"}), result([operation(Type="value")]),
             result([operation(Version=99)]), result([operation(state={})]), result([operation(state=[])]),
             result([operation(state="scalar")])]
    for status in ["ready", "awaiting", "canceling", "completed", "failed", "canceled", "invalid"]:
        for state in [
            {}, {"TerminalError": "owned failure"},
            {"OutPath": "/inert/out", "ErrPath": "/inert/err", "ProcessGroupID": 12345},
            {"Result": {}}, {"Result": {"Out": "stdout\n", "Err": "stderr\n", "ExitCode": 7}},
        ]:
            rows.append(result([operation(status, state)]))
    for elapsed in [0, 1.9999, 2, 2.05, 9.95, 10, 59.5, 60, 82.1, 3600.5, 14400]:
        rows.append(result([operation(state={"Input": {"Timeout": elapsed}, "Result": {"Out": "stdout\r\n", "Err": "stderr\n\n", "TimedOut": True, "Duration": elapsed}})]))
    for state in [{}, {"Input": {"Command": "never printed"}}, {"OutPath": "/inert/out"}, {"ErrPath": "/inert/err"},
                  {"ProcessGroupID": 12345}, {"OutPath": "/inert/界", "ErrPath": "/inert/err", "ProcessGroupID": 12345},
                  {"StartedAt": "2026-11-10T00:00:00Z", "OutPath": "/inert/out"}]:
        rows.append(result([operation("awaiting", state)], action="running"))
    rows += [result(action="running"), result([operation(), operation()], action="running"), result([operation(Type="value")], action="running")]
    rng = random.Random(20261110)
    for _ in range(80):
        rows.append(translate({"command": rng.choice(["printf x", "  x\n", "", "界"]),
                               "timeout": rng.choice([None, 0, "", rng.uniform(0, 14400)]),
                               "max_output_length": rng.choice([1, 3, 80, 40000, "1000"])}))
    for _ in range(96):
        digits = "".join(rng.choice("0123456789abcdef") for _ in range(rng.randrange(1, 32)))
        split = rng.randrange(len(digits) + 1)
        value = f"0x{digits[:split]}.{digits[split:]}p{rng.randrange(-1100, 10)}"
        rows.append(translate({"command": "true", "timeout": value}))
    return rows


MODAL = re.compile(
    r'^(?:decode Bash arguments: |decode Bash argument "(?:command|timeout)": |bash argument: decode max_output_length: )'
    r'json: (?:cannot|unable to) unmarshal JSON (?:array|object|string|number|boolean) into Go '
    r'(?:map\[string\]jsontext\.Value|string|toolarg\.Number)(?:: want a number, such as 100, not .*)?$'
    r'|^decode Bash operation "(?P<operation_id>[^"\n]*)" state: decode shell operation "(?P=operation_id)" state: '
    r'json: (?:cannot|unable to) unmarshal JSON (?:array|string|boolean|number) into Go operation\.ShellState$',
    re.DOTALL,
)


def member(text, wanted):
    decoder, offset = json.JSONDecoder(), 1
    if not text.startswith("{"):
        return None
    try:
        while True:
            while text[offset].isspace():
                offset += 1
            name, offset = decoder.raw_decode(text, offset)
            while text[offset].isspace():
                offset += 1
            if text[offset] != ":":
                return None
            offset += 1
            while text[offset].isspace():
                offset += 1
            start = offset
            value, offset = decoder.raw_decode(text, offset)
            if name == wanted:
                return text[start:offset], value
            while text[offset].isspace():
                offset += 1
            if text[offset] != ",":
                return None
            offset += 1
    except (ValueError, IndexError):
        return None


def bounded(message, limit):
    if len(message) <= limit:
        return {"Error": message}
    head, tail = message[:limit // 2], message[len(message) - (limit - limit // 2):]
    skipped = len(message.encode()) - len(head.encode()) - len(tail.encode())
    return {"Error": f"{head}...{skipped} bytes truncated...{tail}", "ErrorTruncated": True}


def clipped_modal_variants(request):
    # Reconstruct only the source-proven typed diagnostics from their original
    # raw token, then independently apply Go's rune/byte clipping to BOTH modals.
    # Never adjust a truncation count or an arbitrary error to force equality.
    if request.get("Action") != "translate" or request.get("Name") != "Bash":
        return
    text = request.get("Call", {}).get("Arguments", "").strip()
    limit_value = member(text, "max_output_length")
    try:
        limit = int(limit_value[1]) if limit_value else 40000
    except (ValueError, TypeError, OverflowError):
        return
    if not 1 <= limit <= 1000000:
        return
    for key in ["command", "timeout"]:
        item = member(text, key)
        if item is None or item[1] is None:
            continue
        raw, value = item
        shape = "string" if raw.startswith('"') else "array" if raw.startswith("[") else "object" if raw.startswith("{") else "boolean" if isinstance(value, bool) else "number"
        if key == "command":
            if shape == "string":
                continue
            message = f'decode Bash argument "command": json: cannot unmarshal JSON {shape} into Go string'
        else:
            clipped = raw if len(raw.encode()) <= 40 else raw.encode()[:40].decode("utf-8", errors="replace") + "…"
            message = f'decode Bash argument "timeout": json: cannot unmarshal JSON {shape} into Go toolarg.Number: want a number, such as 100, not {clipped}'
        yield bounded(message, limit), bounded(message.replace("json: cannot unmarshal", "json: unable to unmarshal", 1), limit)


def equivalent(go, rust, request=None):
    if go == rust:
        return True
    candidate = copy.deepcopy(rust)
    paths = [(candidate.get("Status") or {}, "Error"), (candidate, "Error")]
    changed = False
    for container, key in paths:
        value = container.get(key, "")
        if isinstance(value, str) and MODAL.fullmatch(value):
            container[key] = value.replace("json: cannot unmarshal", "json: unable to unmarshal", 1)
            changed = True
    if changed and go == candidate:
        return True
    for canonical, alternate in clipped_modal_variants(request or {}):
        if rust.get("Status") == canonical:
            candidate = {**rust, "Status": alternate}
            if go == candidate:
                return True
    return False


def self_test():
    canonical = {"Status": {"Error": "decode Bash argument \"command\": json: cannot unmarshal JSON array into Go string"}, "Specs": []}
    alternate = copy.deepcopy(canonical)
    alternate["Status"]["Error"] = alternate["Status"]["Error"].replace("cannot", "unable to")
    assert equivalent(alternate, canonical)
    for wrong in ["decode Other argument", "Go int", "random cannot text"]:
        bad = copy.deepcopy(alternate)
        bad["Status"]["Error"] += wrong
        assert not equivalent(bad, canonical)
    bad = copy.deepcopy(alternate)
    bad["Specs"] = ["altered opaque payload"]
    assert not equivalent(bad, canonical)
    nested = {"Status": None, "Specs": [], "Result": None, "Running": "", "Error": 'decode Bash operation "owned" state: decode shell operation "owned" state: json: cannot unmarshal JSON array into Go operation.ShellState'}
    other = {**nested, "Error": nested["Error"].replace("json: cannot", "json: unable to")}
    assert equivalent(other, nested)
    wrong = {**other, "Error": other["Error"].replace('decode shell operation "owned"', 'decode shell operation "other"')}
    assert not equivalent(wrong, nested)
    wrong = {**other, "Error": other["Error"] + " altered context"}
    assert not equivalent(wrong, nested)
    request = translate({"command": "true", "timeout": "", "max_output_length": 3})
    original, other = next(clipped_modal_variants(request))
    assert equivalent({"Status": other, "Specs": []}, {"Status": original, "Specs": []}, request)
    corrupted = copy.deepcopy(other)
    corrupted["Error"] = corrupted["Error"].replace("bytes", "byte")
    assert not equivalent({"Status": corrupted, "Specs": []}, {"Status": original, "Specs": []}, request)
    assert not equivalent({"Status": other, "Specs": ["altered"]}, {"Status": original, "Specs": []}, request)


def main():
    self_test()
    rows = cases()
    target = Path(os.environ.get("CARGO_TARGET_DIR", ROOT / "tmp/rust-rewrite-target"))
    subprocess.run(["go", "build", "-o", "tmp/kou-translator-oracle", "./cmd/kou-translator-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-q", "-p", "kou-runtime", "--example", "translator_oracle"], cwd=ROOT, check=True)
    payload = "".join(json.dumps(row, ensure_ascii=False) + "\n" for row in rows)
    def run(command):
        reply = subprocess.run(command, input=payload, text=True, capture_output=True, cwd=ROOT, check=True)
        values = [json.loads(line) for line in reply.stdout.splitlines()]
        assert len(values) == len(rows)
        return values
    go, rust = run([str(ROOT / "tmp/kou-translator-oracle")]), run([str(target / "debug/examples/translator_oracle")])
    failures = 0
    for index, (request, expected, actual) in enumerate(zip(rows, go, rust, strict=True)):
        if not equivalent(expected, actual, request):
            failures += 1
            print(f"FAIL {index}: {json.dumps(request, ensure_ascii=False)}\nGo: {expected}\nRust: {actual}")
            if failures >= 12:
                break
    assert failures == 0, f"{failures} translator projections differ"
    print(f"translator parity: {len(rows)} pure Bash argument/result/running projections passed; no operations executed")


if __name__ == "__main__":
    main()
