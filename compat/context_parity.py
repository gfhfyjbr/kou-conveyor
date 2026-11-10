#!/usr/bin/env python3
"""Compare the actual pure Go/Rust builders; no filesystem or provider effects."""
import base64
import json
import os
from pathlib import Path
import random
import struct
import subprocess
import zlib

ROOT = Path(__file__).resolve().parents[1]


def item(kind, **data):
    return {"Type": kind, "Data": data}


def message(text, role="assistant", **fields):
    return item("message", Role=role, Text=text, **fields)


def response(*items, **fields):
    return {"Action": "response", "Response": {"Output": list(items), **fields}}


def external(text, **fields):
    return {"Action": "input", "Input": {"ID": "input", "Kind": "external", "Payload": text}, **fields}


def action(name, **fields):
    return {"Action": name, **fields}


def result(call, text="whole result " * 200, **fields):
    return action("result", CallID=call, Output=[{"Kind": "text", "Value": text}], **fields)


def compact(text="<review>checked</review><summary>kept task</summary>", **fields):
    return action("compact", Response={"Output": [message(text)]}, **fields)


def turns(count=7):
    steps = []
    for index in range(count):
        call = f"call-{index}"
        steps += [external(f"question {index}"), action("commit"), response(item("reasoning", Raw={"type": "thinking", "thinking": "why " * 100, "signature": "sig"}), message(f"answer {index}"), item("tool_call", CallID=call, Name=["Read", "TranscriptSearch", "Write"][index % 3], Arguments=json.dumps({"path": f"file-{index}.rs"}))), result(call, "result " * (index * 300 + 100)), action("commit")]
    return steps


def png(width, height):
    def chunk(name, body):
        return struct.pack(">I", len(body)) + name + body + struct.pack(">I", zlib.crc32(name + body))
    data = b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", width, height, 8, 0, 0, 0, 0)) + chunk(b"IDAT", zlib.compress((b"\0" * (width + 1)) * height)) + chunk(b"IEND", b"")
    return "data:image/png;base64," + base64.b64encode(data).decode()


def cases():
    yield "empty", {"Steps": [action("build")]}
    yield "pending-response-order", {"Steps": [external("first"), action("commit"), external("arrived during response"), response(message("first answer")), action("build"), action("commit"), action("build")]}
    yield "tools-model-skills", {"Skills": [{"Name": "a<&\"'\n", "Description": "d<&\t", "Path": "/skills/<x>"}, {"Name": "manual", "Manual": True}], "Tools": [{"Type": "function", "Name": "Echo", "Description": "desc", "Parameters": {"z": "ю", "a": {"deep": 1}}}], "Steps": [action("model", Model={"ID": "claude-opus-5", "ReasoningEffort": "max"}), action("system", System="work"), external("hello"), action("control", Control={"Mode": "settings", "Parameters": {"ReasoningEffort": "low"}}), action("build"), action("skills", Skills=[]), action("tools", Tools=[]), action("build")]}
    for data in ["hello", "", {"Text": "with image", "Images": [{"URL": "https://example.invalid/a", "Label": "shot"}]}, {"Text": "linked", "Files": [{"Path": "a b'c.rs", "From": 3, "To": 4, "Lines": 30, "Size": 2048, "Content": "alpha\nbeta\n"}, {"Path": "/tmp/-dir", "Directory": True, "Lines": 3, "From": 1, "To": 2, "Content": "a\nb\n"}]}]:
        yield f"external-{str(data)[:45]}", {"Steps": [external(data), action("build"), action("commit"), response(message("answer")), compact(), action("build")]}
    file_cases = [
        {"Path": "empty"}, {"Path": "img.PNG", "Binary": True, "Size": 2000}, {"Path": "bad", "Error": "missing... "},
        {"Path": "-file", "Size": 1024, "Lines": 10}, {"Path": "~file", "Size": 1024},
        {"Path": "x", "Size": 1, "From": 1, "To": 1, "Lines": 1, "Content": "one"},
        {"Path": "x", "Size": 1, "From": 4, "To": 6, "Lines": 6, "Content": "last\n\n"},
        {"Path": "x", "Size": 1, "From": 1, "To": 2, "Content": "unknown\nlast"},
        {"Path": "a", "Directory": True}, {"Path": "a", "Directory": True, "Lines": 1, "From": 1, "To": 1, "Content": "b\n"},
        {"Path": "a\"<b>&\n", "Label": "label\t\"<&>", "Size": 1},
    ]
    for index, file in enumerate(file_cases):
        yield f"linked-{index}", {"Steps": [external({"Text": "read", "Files": [file]}), action("build")]}
    yield "running-updates", {"Steps": [response(item("tool_call", CallID="call", Name="Bash", Arguments="{}")), result("call", "first", Running=True), result("other", "other"), result("call", "  current  ", Running=True), action("build"), action("commit"), result("call", "completed"), result("call", "duplicate"), action("build")]}
    for budget in [0, 1, 100, 1000, 5000, 20000]:
        for keep in [0, 2, 20]:
            for head in [0, 160]:
                yield f"prune-{budget}-{keep}-{head}", {"Pruning": {"Budget": budget, "Keep": keep, "Bytes": head, "First": ["TranscriptSearch"], "ThinkingTurns": 1}, "Steps": turns() + [action("build"), action("build"), compact(), action("checkpoint"), action("restart"), action("build"), compact(), action("checkpoint")]}
    for keep in [0, 1, 3, 8]:
        for budget in [0, 1000, 12000]:
            yield f"mask-{keep}-{budget}", {"Masking": {"Results": keep}, "Pruning": {"Budget": budget, "Keep": 1, "Bytes": 10, "ThinkingTurns": 2}, "Steps": turns() + [action("build"), action("build"), compact(), action("checkpoint"), action("restart"), action("build")]}
    for usage in [{}, {"InputTokens": 20000, "OutputTokens": 500}, {"InputTokens": -1, "OutputTokens": 0}]:
        for resumed in [False, True]:
            steps = turns(4) + [response(message("latest"), Usage=usage)]
            if resumed:
                steps.append(action("resumed"))
            yield f"count-{usage}-{resumed}", {"Pruning": {"Budget": 3000, "Keep": 1, "ThinkingTurns": 1}, "Steps": steps + [action("build"), external("pending"), action("build")]}
    for text in ["", "plain", "<review>only", "<analysis>old</analysis><summary>new</summary>", "<review>x</review>left<summary>body</summary>tail", "<summary>partial", "<summary></summary>", "<review>done</review>", "<review>x</analysis>other", "<summary>first</summary><summary>second</summary>"]:
        yield f"summary-{text}", {"Steps": turns(1) + [compact(text), action("build")]}
    for budget in [0, 1, 100, 1500, 4000, 10000]:
        yield f"fit-{budget}", {"Steps": turns() + [action("compaction-request", Focus="  exact focus  ", Budget=budget), compact(), result("call-0", "late"), external("pending"), action("compaction-request", Budget=budget), action("build")]}
    yield "compaction-live-pending-refresh", {"Steps": [external("answered"), action("commit"), response(item("tool_call", CallID="live", Name="Bash", Arguments="x\n y")), result("live", Running=True), action("commit"), external("pending"), action("commit"), external("staged"), action("refresh", CallID="live", Output=[{"Kind": "text", "Value": "progress"}]), compact(), action("checkpoint", Live=["live"]), action("build"), action("restart"), action("build"), result("live", "finished"), action("commit"), action("build")]}
    for notes in ["", "old notes", "я" * 18000, "a\n" * 17000]:
        yield f"notes-{len(notes)}", {"NotesPath": "/synthetic/notes", "Notes": notes, "Summaries": "/synthetic/summaries", "Transcript": "/synthetic/log", "Steps": turns(1) + [compact(), action("checkpoint"), action("notes", Notes="new notes"), action("restart"), action("notes-update"), action("control", Control={"Mode": "heartbeat", "Reason": "[harness] Your notes changed since the conversation last showed them. They are in /synthetic/notes, and read now:\n<notes>\nnew notes\n</notes>"}), action("notes-update"), action("commit"), action("build")]}
    for name in ["Read", "mcp__server__Read", "Edit", "Write", "apply_patch"]:
        for failed in [False, True]:
            args = {"path": "touched.rs", "patch": "*** Begin Patch\n*** Add File: added.rs\n+x\n*** Move to: moved.rs\n*** End Patch"}
            yield f"touch-{name}-{failed}", {"Masking": {"Results": 1}, "Steps": [response(item("tool_call", CallID="x", Name=name, Arguments=json.dumps(args))), result("x", ("Error:" if failed else "done") + " details " * 2000), result("last", "latest"), action("commit"), action("build"), compact(), action("checkpoint"), action("restart"), compact(), action("build")]}
    long_steps = []
    for index in range(80):
        text = f"prompt {index}: " + "я" * 1000 + ("long input " * 5000 if index == 79 else "")
        long_steps += [external(text), action("commit"), response(message("answer"))]
    long_steps += [compact(), action("checkpoint"), action("restart")]
    for index in range(3):
        long_steps += [external(f"after {index} " + "new " * (index * 3000)), action("commit"), response(message("answer")), compact(), action("checkpoint"), action("restart"), action("build")]
    yield "bounded-answered-future-compactions", {"Steps": long_steps}
    for count in [1, 40, 41, 46]:
        steps = []
        for index in range(count):
            steps += [external({"Text": f"shot {index}", "Images": [{"URL": "https://example.invalid/image", "Label": f"image {index}"}]}), action("commit")]
        yield f"image-window-{count}", {"Steps": steps + [action("build"), action("build")]}
    for width, height in [(1, 1), (300, 250), (1500, 1000), (5152, 2576)]:
        url = png(width, height)
        yield f"image-estimate-{width}-{height}", {"Steps": [external({"Images": [{"URL": url}]}), action("build")]}
    raw_png = base64.b64decode(png(300, 250).split(",")[1])
    header_cases = [("png-valid", raw_png)]
    for at in range(8, 33):
        changed = bytearray(raw_png)
        changed[at] ^= 1
        header_cases.append((f"png-byte-{at}", changed))
    gif = b"GIF89a" + struct.pack("<HH", 300, 250) + b"\x80\0\0" + b"\0" * 6
    for end in [10, 13, 14, 19]:
        header_cases.append((f"gif-prefix-{end}", gif[:end]))
    sof = b"\x08" + struct.pack(">HH", 250, 300) + b"\x01\x01\x11\x00"
    for jfif in [False, True]:
        start = b"\xff\xd8" + (b"\xff\xe0\x00\x07JFIF\0" if jfif else b"")
        full = start + b"\xff\xc0\x00\x0b" + sof
        for end in [b"", b"\xff\xda\x00\x02", b"\xff\xd9"]:
            header_cases.append((f"jpeg-{jfif}-{end.hex()}", full + end))
        for at in range(len(sof)):
            changed = bytearray(sof)
            changed[at] ^= 1
            header_cases.append((f"jpeg-sof-{jfif}-{at}", start + b"\xff\xc0\x00\x0b" + changed + b"\xff\xda\x00\x02"))
    def webp(kind, body):
        chunk = kind + struct.pack("<I", len(body)) + body + (b"\0" if len(body) & 1 else b"")
        return b"RIFF" + struct.pack("<I", len(chunk) + 4) + b"WEBP" + chunk
    vp8x = b"\0" * 4 + (299).to_bytes(3, "little") + (249).to_bytes(3, "little")
    vp8l = b"\x2f" + (299 | (249 << 14)).to_bytes(4, "little")
    vp8 = b"\0\0\0\x9d\x01\x2a" + struct.pack("<HH", 300, 250)
    for kind, body in [(b"VP8X", vp8x), (b"VP8L", vp8l), (b"VP8 ", vp8)]:
        for end in range(len(body) + 1):
            header_cases.append((f"webp-{kind.decode()}-{end}", webp(kind, body[:end])))
    for name, data in header_cases:
        url = "data:image/unknown;base64," + base64.b64encode(data).decode()
        yield "header-" + name, {"Steps": [external({"Images": [{"URL": url}]}), action("build")]}
    for state in [{}, {"Version": 2}, {"Version": 1, "Committed": [message("not user")]}, {"Version": 1, "Committed": [message("summary", "user")], "Answered": ["a" * 40001]}]:
        yield "invalid-checkpoint-" + str(state)[:60], {"Steps": [external("unchanged"), action("restore", Encoded=state), action("build")]}
    for payload in [None, [], 3, {"Images": [{"URL": " "}]}, {"Files": [{"Path": ""}]}, {"Files": [{"Path": "x", "From": 3, "To": 1}]}]:
        yield "invalid-input-" + str(payload), {"Steps": [external(payload), action("build")]}
    rng = random.Random(20261110)
    for index in range(80):
        steps = []
        for turn in range(rng.randrange(1, 8)):
            call = f"c{turn}"
            steps += [external(f"question {turn}"), action("commit"), response(item("reasoning", Summary=["why " * rng.randrange(1, 100)]), item("tool_call", CallID=call, Name="Read", Arguments="{}"), Usage={"InputTokens": rng.randrange(3000)}), result(call, "data " * rng.randrange(2000), Running=rng.choice([False, False, True])), action("commit")]
            if rng.choice([True, False]): steps.append(action("build"))
        steps += [action("build"), compact(), action("checkpoint"), action("restart"), action("build")]
        yield f"seeded-{index}", {"Pruning": {"Budget": rng.randrange(500, 5000), "Keep": rng.randrange(3), "Bytes": rng.randrange(100), "ThinkingTurns": rng.randrange(3)}, "Masking": {"Results": rng.randrange(4)}, "Steps": steps}


def diff(a, b, path="$"):
    if type(a) is not type(b):
        return f"{path}: Go {a!r}, Rust {b!r}"
    if isinstance(a, dict):
        if a.keys() != b.keys():
            return f"{path}: Go keys {a.keys()}, Rust keys {b.keys()}"
        return next((why for key in a if (why := diff(a[key], b[key], f"{path}.{key}"))), None)
    if isinstance(a, list):
        if len(a) != len(b):
            return f"{path}: Go length {len(a)}, Rust {len(b)}"
        return next((why for i, pair in enumerate(zip(a, b)) if (why := diff(*pair, f"{path}[{i}]"))), None)
    if a != b:
        if isinstance(a, str):
            at = next((i for i, (x, y) in enumerate(zip(a, b)) if x != y), min(len(a), len(b)))
            return f"{path}: at {at}, Go {a[max(0, at-50):at+150]!r}, Rust {b[max(0, at-50):at+150]!r} (lengths {len(a)}/{len(b)})"
        return f"{path}: Go {a!r}, Rust {b!r}"
    return None


class Oracle:
    def __init__(self, command):
        self.process = subprocess.Popen(command, cwd=ROOT, stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1)
    def call(self, value):
        self.process.stdin.write(json.dumps(value, ensure_ascii=False) + "\n")
        self.process.stdin.flush()
        line = self.process.stdout.readline()
        if not line:
            raise RuntimeError(f"oracle exited: {self.process.poll()}")
        return json.loads(line)
    def close(self):
        self.process.stdin.close()
        self.process.wait(timeout=30)
        if self.process.returncode:
            raise RuntimeError(f"oracle exit {self.process.returncode}")


def main():
    bindir = ROOT / "tmp/rust-context-oracle"
    bindir.mkdir(parents=True, exist_ok=True)
    go_bin = bindir / "go-context-oracle"
    subprocess.run(["go", "build", "-o", str(go_bin), "./cmd/kou-context-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-engine", "--example", "context_oracle"], cwd=ROOT, env={**os.environ, "CARGO_INCREMENTAL": "0"}, check=True)
    go = Oracle([str(go_bin)])
    rust = Oracle([str(ROOT / "tmp/rust-rewrite-target/debug/examples/context_oracle")])
    count = 0
    try:
        for name, value in cases():
            expected, actual = go.call(value), rust.call(value)
            why = diff(expected, actual)
            if why:
                raise AssertionError(f"{name}: {why}")
            for checkpoint in (entry["Checkpoint"] for entry in expected if "Checkpoint" in entry):
                if checkpoint.get("Committed"):
                    continuation = {**value, "Steps": [action("restore", Encoded=checkpoint), action("build"), compact("next summary"), action("checkpoint")]}
                    why = diff(go.call(continuation), rust.call(continuation))
                    if why: raise AssertionError(f"Go checkpoint -> Rust {name}: {why}")
                    # Semantic checkpoint JSON must also be consumable by the actual Go builder.
                    checkpoint2 = next(entry["Checkpoint"] for entry in actual if "Checkpoint" in entry and entry["Checkpoint"] == checkpoint)
                    continuation["Steps"][0]["Encoded"] = checkpoint2
                    why = diff(go.call(continuation), rust.call(continuation))
                    if why: raise AssertionError(f"Rust checkpoint -> Go {name}: {why}")
            count += 1
        print(f"PASS: {count} context scenarios, exact requests/reports/estimates/memory and bidirectional checkpoints; no providers")
    finally:
        go.close()
        rust.close()


if __name__ == "__main__":
    main()
