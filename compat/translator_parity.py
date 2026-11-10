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
    for value in [9007199254740992, 9007199254740993, 9007199254740994, "9007199254740994", 1e18, -9007199254740994]:
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


FILE_TOOLS = [
    ("read", "Read", {"path": "file"}),
    ("edit", "Edit", {"path": "file", "old_string": "old", "new_string": "new"}),
    ("write", "Write", {"path": "file", "content": "body\n"}),
    ("apply_patch", "apply_patch", {"input": " raw patch \n"}),
    ("transcript_search", "TranscriptSearch", {"query": " needle "}),
]


def file_translate(kind, name, arguments, config=None):
    return {
        "Action": "translate", "Kind": kind, "Name": name,
        "File": config if config is not None else {"Directory": "/inert/work/", "Transcript": "/inert/transcript"},
        "Call": {"CallID": "call", "Name": name, "Arguments": arguments if isinstance(arguments, str) else json.dumps(arguments, ensure_ascii=False)},
    }


def file_operation(status="completed", **state):
    return {"ID": "owned", "Type": "file", "Version": 1, "Status": status,
            "State": {"Action": "read", "Path": "/inert/file", "Result": {"Text": "same\r\n"}, **state}}


def file_result(operations=(), status=None, kind="read", name="Read", action="result"):
    return {"Action": action, "Kind": kind, "Name": name, "Call": {"CallID": "call"},
            "Status": status or {}, "Operations": list(operations)}


def read_step(call_id, current=None, status=None):
    return {"CallID": call_id, "Status": status or {}, "Operations": [current if current is not None else file_operation()]}


def file_sequence(steps, kind="read", name="Read"):
    return {"Action": "sequence", "Kind": kind, "Name": name, "Steps": steps}


def file_cases():
    rows = []
    invalid_roots = ["", " \n\t", "{", "null", "[]", "true", "42", '"text"', '{} x',
                     '{"path":"a","path":"b"}', '{"unknown":{"x":1,"x":2}}',
                     '{"path":"a","p\\u0061th":"b"}', '{"unknown":"\\ud800"}',
                     '{"path":"a",}', '{"unknown":[1,]}']
    for kind, name, valid in FILE_TOOLS:
        rows += [file_translate(kind, name, raw) for raw in invalid_roots]
        rows.append(file_translate(kind, name, valid))
        rows.append(file_translate(kind, "Alias-界", valid))
        rows.append(file_translate(kind, name, {**valid, "max_output_length": False, "Unknown": {"nested": [None, 1]}}))
        for key in ["path", "file_path", "content", "input", "patch", "query", "old_string", "new_string", "replace_all"]:
            for value in [None, "", " \n", 42, True, [], {}]:
                rows.append(file_translate(kind, name, {**valid, key: value}))
        # All typed decoding precedes count/path/kind validation, in source order.
        for invalid in [
            {"path": 42, "offset": "soon"}, {"offset": "soon", "path": 42},
            {"offset": 1.5, "path": 42}, {"replace_all": "true", "new_string": []},
            {"new_string": [], "replace_all": "true"}, {"limit": -1, "offset": -2},
            {"offset": 1.5, "limit": "soon"}, {"offset": -1, "path": "a\0b"},
            {"path": "a\0b", "query": "x", "input": "patch"},
        ]:
            rows.append(file_translate(kind, name, invalid))
        for config in [{}, {"Directory": "relative/", "Transcript": "raw/../log"},
                       {"Directory": "/", "Transcript": " "}, {"Directory": "/inert/../work//", "Transcript": "/inert//log"}]:
            rows.append(file_translate(kind, name, valid, config))
    for kind, name, valid in FILE_TOOLS[:3]:
        for path in [None, "", " \n", " a/../b ", "/absolute/../file", "//absolute//file", "界/../é", "a\0b"]:
            for fallback in [None, " fallback/../name "]:
                rows.append(file_translate(kind, name, {**valid, "path": path, "file_path": fallback}))
    for kind, name, valid in FILE_TOOLS:
        rows.append(file_translate(kind, name, '{"unknown":1e999,' + json.dumps(valid, ensure_ascii=False)[1:]))
        rows.append(file_translate(kind, name, {**valid, "Path": 42, "OFFSET": "bad", "Replace_All": []}))
    numbers = [None, "", " ", 0, -0.0, "0", -1, -0.5, 1.5, 1e-5, 1e-4, 1e6 + 0.5,
               1e100, -1e100, 9007199254740991, 9007199254740992, 9007199254740993, 9007199254740994,
               -9007199254740992, -9007199254740994, "9007199254740994", 9223372036854774784, 9223372036854775807, -9223372036854775808,
               -9223372036854777856, "9223372036854774784", "9223372036854775808", " 12 ",
               "1_000", "0X_1.fp+2", "0x1p-1074", "0x1p-1075", "0x1p9999", "+1.", ".5",
               "NaN", "Inf", "soon", "界" * 30, True, [], {}]
    for field in ["offset", "limit"]:
        for value in numbers:
            rows.append(file_translate("read", "Read", {"path": "file", field: value}))
        rows.append(file_translate("read", "Read", '{"path":"file","' + field + '":1e999}'))
    for name in ["Alias", "Files.Read", "界"]:
        rows += [file_translate("unknown", name, {"path": "file"}), file_translate("unknown", name, {"path": 42})]
    for args in [{"input": " \n", "patch": " fallback\n"}, {"input": " first\n", "patch": " second\n"},
                 {"input": None, "patch": " fallback"}, {"input": "", "patch": " \t"}]:
        rows.append(file_translate("apply_patch", "PatchAlias", args))
    for query in [None, "", " \n", " needle ", "\\[regex\\]", "\0"]:
        for transcript in ["", " ", "/inert//log"]:
            rows.append(file_translate("transcript_search", "SearchAlias", {"query": query, "limit": "3"}, {"Transcript": transcript}))

    for kind, name, _ in FILE_TOOLS:
        rows += [file_result(kind=kind, name=name), file_result([file_operation(), file_operation()], kind=kind, name=name),
                 file_result(status={"Error": "validation\n", "ErrorTruncated": True}, kind=kind, name=name),
                 file_result([file_operation()], {"Error": "validation"}, kind, name),
                 file_result([file_operation()], {"WaitingFor": ["ignored"], "Success": True}, kind, name)]
        for status in ["ready", "awaiting", "canceling", "completed", "failed", "canceled", "invalid"]:
            for fields in [{"Result": None}, {"Result": {}}, {"Result": {"Text": "exact\r\n\n"}},
                           {"TerminalError": "owned failure"}, {"Result": {"Text": None, "Files": [], "Lines": None, "Truncated": None}}]:
                rows.append(file_result([file_operation(status, **fields)], kind=kind, name=name))
        rows.append(file_result([file_operation()], kind=kind, name=name, action="running"))
    for changes in [{"Type": "shell"}, {"Version": 0}, {"Version": 99}, {"State": None},
                    {"State": []}, {"State": True}, {"State": 12}, {"State": "text"}, {"State": {}},
                    {"State": {"Action": "read"}}, {"State": {"Action": "patch", "Patch": " "}},
                    {"State": {"Action": "unknown"}},
                    {"State": {"Action": "read", "Path": "x", "Offset": -1}}]:
        rows.append(file_result([{**file_operation(), **changes}]))
    missing_state = file_operation()
    del missing_state["State"]
    rows.append(file_result([missing_state]))
    # A derived Serde struct must not accept positional arrays as FileResult.
    for value in [[], ["positional"], ["positional", [], 1, 2, 3, False], "text", 42, True]:
        rows.append(file_result([file_operation(Result=value)]))
    for fields in [{"Text": "界" * 40001}, {"Text": "\0raw\r\n", "Files": ["raw/../file"], "Lines": 2, "From": 1, "To": 2, "Truncated": True}]:
        rows.append(file_result([file_operation(Result=fields)]))
    for action, fields in [("write", {}), ("edit", {}), ("patch", {"Patch": "patch"}),
                           ("transcript_search", {"Query": "needle"})]:
        rows.append(file_result([file_operation(Action=action, **fields)]))
    for call_id in ["", 'call"quoted', "界-call"]:
        row = file_result([file_operation("failed", TerminalError="owned error")])
        row["Call"]["CallID"] = call_id
        rows.append(row)

    rows.append(file_sequence([read_step(value) for value in ["one", "two", "three", "three", "four", "one"]]))
    rows.append(file_sequence([read_step(str(i), file_operation(Result={"Text": text}))
                               for i, text in enumerate(["same", "same", "same", "changed", "changed", "changed", "same", "same", "same"])]))
    rows.append(file_sequence([read_step("one"), read_step("two"), read_step("three"),
                               read_step("three", file_operation(Path="/other/path", Result={"Text": "new current text"})),
                               read_step("four"), read_step("two", file_operation(Result={"Text": "changed"}))]))
    for field, values in [("Path", ["/inert/file", "/inert/./file", "/inert//file", "/inert/file"]),
                          ("Offset", [0, 1, 2, 0]), ("Limit", [0, 1, 2, 0])]:
        rows.append(file_sequence([read_step(f"{index}-{repeat}", file_operation(**{field: value}))
                                   for index, value in enumerate(values) for repeat in range(3)]))
    for kind, name, _ in FILE_TOOLS:
        rows.append(file_sequence([read_step(str(i)) for i in range(5)], kind, name))
    for status in ["ready", "awaiting", "canceling", "failed", "canceled", "invalid"]:
        rows.append(file_sequence([read_step("retry", file_operation(status, TerminalError="failed")),
                                   *[read_step(value) for value in ["retry", "two", "three"]]]))
    rows.append(file_sequence([read_step("retry", file_operation(Result=[])),
                               *[read_step(value) for value in ["retry", "two", "three"]]]))
    for action, fields in [("write", {}), ("edit", {}), ("patch", {"Patch": "patch"}),
                           ("transcript_search", {"Query": "needle"})]:
        rows.append(file_sequence([read_step("retry", file_operation(Action=action, **fields)),
                                   *[read_step(value) for value in ["retry", "two", "three"]]]))
    rng = random.Random(20261111)
    for _ in range(32):
        steps = [read_step(str(rng.randrange(8)), file_operation(Path=rng.choice(["/inert/file", "/inert/./file"]),
                  Offset=rng.randrange(2), Limit=rng.randrange(2), Result={"Text": rng.choice(["same", "other", "界\n"])}))
                 for _ in range(40)]
        rows.append(file_sequence(steps))
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


def file_diagnostic(value, request):
    if not isinstance(value, str) or not request.get("Kind"):
        return False
    name = re.escape(request["Name"])
    modal = r'json: (?:cannot|unable to) unmarshal JSON (?:array|object|string|number|boolean) into Go '
    if request.get("Action") == "translate":
        fields = (r'(?:string within "/(?:path|file_path|content|input|patch|query|old_string|new_string)"'
                  r'|bool within "/replace_all"'
                  r'|toolarg\.Number within "/(?:offset|limit)": want a number, such as 100, not .*)')
        return re.fullmatch(f'decode {name} arguments: ' + modal + fields, value, re.DOTALL) is not None
    operations = request.get("Operations", [])
    if request.get("Action") != "result" or len(operations) != 1:
        return False
    call_id = re.escape(json.dumps(request["Call"]["CallID"], ensure_ascii=False))
    operation_id = re.escape(json.dumps(operations[0]["ID"], ensure_ascii=False))
    prefix = f'decode {name} call {call_id} result: decode file operation {operation_id} state: '
    fields = r'(?:operation\.FileState|operation\.FileResult within "/Result")'
    return re.fullmatch(prefix + modal + fields, value) is not None


def equivalent(go, rust, request=None):
    if go == rust:
        return True
    request = request or {}
    candidate = copy.deepcopy(rust)
    if request.get("Action") == "sequence":
        expected, actual, steps = go.get("Sequence", []), rust.get("Sequence", []), request.get("Steps", [])
        if len(expected) != len(actual) or len(actual) != len(steps):
            return False
        for left, right, step in zip(expected, actual, steps, strict=True):
            part = {**request, "Action": "result", "Call": {"CallID": step["CallID"]},
                    "Status": step.get("Status", {}), "Operations": step.get("Operations", [])}
            if not equivalent(left, right, part):
                return False
        candidate["Sequence"] = copy.deepcopy(expected)
    for actual, expected, key in [(candidate.get("Status") or {}, go.get("Status") or {}, "Error"), (candidate, go, "Error")]:
        value, other = actual.get(key, ""), expected.get(key, "")
        if (isinstance(value, str) and isinstance(other, str)
                and (MODAL.fullmatch(value) or file_diagnostic(value, request))
                and (MODAL.fullmatch(other) or file_diagnostic(other, request))
                and value.replace("json: unable to unmarshal", "json: cannot unmarshal", 1)
                == other.replace("json: unable to unmarshal", "json: cannot unmarshal", 1)):
            actual[key] = other
    if go == candidate:
        return True
    for canonical, alternate in clipped_modal_variants(request):
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
    request = file_translate("read", "Alias-界", {"path": 42})
    message = 'decode Alias-界 arguments: json: cannot unmarshal JSON number into Go string within "/path"'
    canonical = {"Status": {"Error": message}, "Specs": [], "Sequence": []}
    alternate = copy.deepcopy(canonical)
    alternate["Status"]["Error"] = message.replace("json: cannot", "json: unable to")
    assert equivalent(alternate, canonical, request)
    for before, after in [("Alias-界", "Read"), ("/path", "/file_path"), ("JSON number", "JSON array"), ("Go string", "Go bool")]:
        bad = copy.deepcopy(alternate)
        bad["Status"]["Error"] = bad["Status"]["Error"].replace(before, after)
        assert not equivalent(bad, canonical, request)
    message = 'decode Read call "retry" result: decode file operation "owned" state: json: cannot unmarshal JSON array into Go operation.FileResult within "/Result"'
    canonical = {"Status": None, "Specs": [], "Error": message, "Sequence": []}
    alternate = {**canonical, "Error": message.replace("json: cannot", "json: unable to")}
    request = file_sequence([read_step("retry", file_operation(Result=[]))])
    left, right = {"Sequence": [alternate]}, {"Sequence": [canonical]}
    assert equivalent(left, right, request)
    for before, after in [('call "retry"', 'call "other"'), ('operation "owned"', 'operation "other"'), ("/Result", "/Other")]:
        bad = copy.deepcopy(left)
        bad["Sequence"][0]["Error"] = bad["Sequence"][0]["Error"].replace(before, after)
        assert not equivalent(bad, right, request)
    assert not equivalent({"Sequence": [alternate, alternate]}, right, request)
    assert not equivalent({"Sequence": [{**alternate, "Result": {"opaque": message}}]}, right, request)
    opaque = {"Result": {"Output": [{"Value": message}]}, "Sequence": []}
    assert not equivalent({"Result": {"Output": [{"Value": alternate["Error"]}]}, "Sequence": []}, opaque, request)


def main():
    self_test()
    bash_rows, file_rows = cases(), file_cases()
    rows = bash_rows + file_rows
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
    sequences = [row for row in file_rows if row["Action"] == "sequence"]
    steps = sum(len(row["Steps"]) for row in sequences)
    print(f"translator parity: {len(rows)} pure Go/Rust projections ({len(bash_rows)} Bash, {len(file_rows)} File including {len(sequences)} sequences / {steps} result steps); no operations executed")


if __name__ == "__main__":
    main()
