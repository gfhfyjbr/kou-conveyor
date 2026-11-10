#!/usr/bin/env python3
"""Drive the real Go coordinator's pre-scheduler replay boundary against Rust."""
import copy
import json
import os
from pathlib import Path
import random
import subprocess
from context_parity import Oracle, diff, message, item as llm_item

ROOT = Path(__file__).resolve().parents[1]


def record(kind, data):
    return {"Sequence": 0, "Kind": kind, "Data": data}


def records(items):
    return [{**value, "Sequence": index + 1, "RecordedAt": "2026-11-10T02:00:00Z"} for index, value in enumerate(items)]


def external(text):
    return record("input", {"ID": "input", "Kind": "external", "Payload": text})


def turn(name, kind="model", model="model-a"):
    return record("turn", {"ID": name, "Type": kind, "Model": model})


def response(name, *output, **fields):
    return record("model_response", {"TurnID": name, "Response": {"Output": list(output), **fields}})


def status(turn_id, call_id, waiting=(), operations=(), error=""):
    return record("tool_call_status", {"TurnID": turn_id, "CallID": call_id, "Status": {"WaitingFor": list(waiting), "Error": error}, "Operations": list(operations)})


def operation(name, status="completed"):
    return {"ID": name, "Type": "process", "Version": 1, "Status": status, "State": {}}


def ordinary(name="t1", tool="Read", complete=True):
    values = [external("question"), turn(name), response(name, message("thinking about it"), llm_item("tool_call", CallID="c1", Name=tool, Arguments='{"path":"file.rs"}'))]
    if complete:
        values += [status(name, "c1", ["b", "a"], [operation("a"), operation("b")])]
    return values


def cases():
    yield "empty", {"Items": []}
    yield "input-before-first-turn", {"Items": records([external("pending")])}
    for complete in [False, True]:
        for name in ["Read", "Write", "Describe", "Reject", "Missing"]:
            yield f"call-{name}-{complete}", {"Items": records(ordinary(tool=name, complete=complete))}
    for name in ["Read", "Describe", "Missing"]:
        for state in ["pending", "running", "completed", "failed", "canceled"]:
            yield f"operations-{name}-{state}", {"Items": records(ordinary(tool=name, complete=False) + [status("t1", "c1", ["a"], [operation("a", state)])])}
    for waiting, operations in [(["unknown"], []), (["b", "a"], [operation("a")]), ([], [operation("unwaited", "running")]), ([], []), (["a", "a"], [operation("a")])]:
        yield f"waiting-{waiting}-{operations}", {"Items": records(ordinary(complete=False) + [status("t1", "c1", waiting, operations)])}
    for tool in ["Missing", "Read", "Reject"]:
        for error in ["Error: unavailable", "unknown tool"]:
            yield f"error-only-{tool}-{error}", {"Items": records(ordinary(tool=tool, complete=False) + [status("t1", "c1", error=error)])}
    yield "orphan-status", {"Items": records([status("missing-turn", "missing", ["unknown"], [operation("unknown")])])}
    yield "status-updates", {"Items": records(ordinary(tool="Describe", complete=False) + [status("t1", "c1", ["a"], [operation("a", "running")]), status("t1", "c1", ["a"], [operation("a", "completed")]), turn("t2"), response("t2", message("done"))])}
    yield "fork-clears-live-calls", {"Items": records(ordinary(complete=False) + [record("fork", {"ParentID": "parent", "PreviousTurnID": "t1"}), status("t1", "c1", ["a"], [operation("a")])])}
    yield "model-history", {"Items": records([external("question"), turn("t1", model="old"), response("t1", llm_item("reasoning", Raw={"thinking":"old","signature":"sig"}), message("old answer")), turn("t2", model="new"), response("t2", {**message("new answer"), "Model":"already-tagged"})]), "Model": {"ID":"new"}}
    for summary in ["<summary>kept</summary>", "", "<review>only", "plain summary"]:
        yield f"compact-{summary}", {"Items": records(ordinary() + [turn("compact", "compaction"), response("compact", message(summary)), external("next")])}
    yield "saved-compaction-message", {"Items": records(ordinary() + [turn("compact", "compaction"), record("model_response", {"TurnID":"compact", "Response":{"Output":[message("summary")]}, "Compacted":{"Message":"saved exactly", "Notes":"saved notes"}})])}
    for mode in ["hard", "when_idle", "heartbeat", "settings", "compact"]:
        control = {"Mode": mode, "Reason":"[harness] resume run note"}
        if mode == "settings": control["Parameters"] = {"ReasoningEffort":"high"}
        yield f"control-{mode}", {"Items": records(ordinary() + [record("input", {"ID":"control", "Kind":"control", "Payload":control})])}
    for masking in [0, 1, 3]:
        for thinking in [0, 1, 2]:
            values = []
            for index in range(8):
                name = f"t{index}"
                values += [external("question"), turn(name), response(name, llm_item("reasoning", Summary=["thinking " * 300]), llm_item("tool_call", CallID=f"c{index}", Name="Read", Arguments='{}')), status(name, f"c{index}", ["op"], [operation("op")])]
            yield f"prune-{masking}-{thinking}", {"Items": records(values), "Masking":{"Results":masking}, "Pruning":{"Budget":2000,"Keep":1,"Bytes":20,"ThinkingTurns":thinking}}
    for name in ["Write", "mcp__server__Write", "apply_patch", "Edit"]:
        yield f"finished-file-counter-{name}", {"Available": [name], "Items": records(ordinary(tool=name, complete=False) + [status("t1", "c1", ["op"], [operation("op")])])}
    for mode, reason in [("heartbeat", "[harness verification] failed"), ("heartbeat", "[harness] The conversation will be compacted soon. reminder"), ("heartbeat", "ordinary"), ("hard", ""), ("hard", "budget"), ("when_idle", "budget")]:
        control = record("input", {"ID": "control", "Kind": "control", "Payload": {"Mode": mode, "Reason": reason}})
        yield f"control-counters-{mode}-{reason}", {"Items": records(ordinary() + [control])}
    yield "verification-orphan-operation", {"Items": records(ordinary() + [status("t1", "verification", ["check"], [{**operation("check", "ready"), "Idempotency": {"harness": "verification"}}])])}
    for gap in range(5):
        history = [external("task"), turn("one", "regular"), response("one", message("reply")), turn("compact-one", "compaction"), response("compact-one", message("<summary>one</summary>"))]
        for n in range(gap):
            history += [turn(f"gap-{n}", "regular"), response(f"gap-{n}", message("ordinary"))]
        yield f"rapid-refill-{gap}", {"Items": records(history + [turn("compact-two", "compaction"), response("compact-two", message("<summary>two</summary>"))])}
    rng = random.Random(20261111)
    for sample in range(60):
        values = []
        for index in range(rng.randrange(1, 12)):
            name = f"t{index}"
            values += [external(f"question {index}"), turn(name)]
            tool = rng.choice(["Read","Write","Bash","Describe"])
            values += [response(name, message("answer"), llm_item("tool_call", CallID=f"c{index}", Name=tool, Arguments='{}'))]
            if rng.choice([True,True,False]): values += [status(name, f"c{index}", [f"op{index}"], [operation(f"op{index}", rng.choice(["completed","failed","running"]))])]
            if rng.randrange(5) == 0: values += [turn(f"compact{index}","compaction"), response(f"compact{index}",message(f"summary {index}"))]
        yield f"seeded-{sample}", {"Items":records(values)}
    yield "checkpoint-export-boundary", {"Items": records(ordinary(complete=False) + [status("t1","c1",["pending"],[operation("pending","running")]),turn("compact","compaction"),response("compact",message("summary"))])}


def main():
    bindir = ROOT / "tmp/rust-replay-oracle"
    bindir.mkdir(parents=True,exist_ok=True)
    go_bin = bindir / "go-replay-oracle"
    subprocess.run(["go","build","-o",str(go_bin),"./cmd/kou-replay-oracle"],cwd=ROOT,check=True)
    subprocess.run(["cargo","build","--locked","-p","kou-engine","--example","replay_oracle"],cwd=ROOT,env={**os.environ,"CARGO_INCREMENTAL":"0"},check=True)
    go = Oracle([str(go_bin)]); rust = Oracle([str(ROOT / "tmp/rust-rewrite-target/debug/examples/replay_oracle")])
    count = imports = 0
    try:
        for name, value in cases():
            value = {**value, "StateProbe": True}
            expected,actual = go.call(value),rust.call(value)
            if why := diff(expected,actual): raise AssertionError(f"{name}: {why}")
            exported = rust.call({**value,"Export":True})
            if exported.get("State") is not None:
                state = exported["State"]
                continuation = {**value,"Checkpoint":{"State":state,"Items":[]}}
                if why := diff(go.call(continuation),rust.call(continuation)): raise AssertionError(f"Rust coordinator checkpoint -> actual Go {name}: {why}")
                for change in ["version","negative-count","wrong-turn","missing-builder","missing-tool"]:
                    changed = copy.deepcopy(state)
                    if change == "version": changed["Version"] = 2
                    elif change == "negative-count": changed["AvailableInputs"] = -1
                    elif change == "wrong-turn": changed["CurrentTurnType"] = "model"
                    elif change == "missing-builder": changed["Builder"] = {}
                    else: continuation["Available"] = []
                    continuation["Checkpoint"]["State"] = changed
                    if why := diff(go.call(continuation),rust.call(continuation)): raise AssertionError(f"checkpoint fallback {name}/{change}: {why}")
                imports += 1
            count += 1
        print(f"PASS: {count} actual coordinator replay scenarios; {imports} native coordinator checkpoint imports/fallback matrices; no tools/providers")
    finally:
        go.close();rust.close()


if __name__ == "__main__": main()
