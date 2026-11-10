#!/usr/bin/env python3
"""Compare live coordinator traces; only generated identity fields are remapped."""
import copy
import json
import os
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def external(identifier="one", text="owned task", delivery=""):
    return {"Kind": "input", "Input": {"ID": identifier, "Kind": "external", "Payload": text, "Delivery": delivery}}


def control(mode="when_idle", identifier="stop"):
    return {"Kind": "input", "Input": {"ID": identifier, "Kind": "control", "Payload": {"Mode": mode, "Reason": "requested stop"}}}


def request():
    return {"Kind": "request"}


def respond(index=0, response=None):
    if response is None:
        response = {"Output": [{"Type": "message", "Data": {"Role": "assistant", "Text": "owned answer"}}]}
    return {"Kind": "respond", "Index": index, "Response": response}


def call(name="Bash", count=1):
    return {"Output": [{"Type": "tool_call", "Data": {"CallID": "c1", "Name": name, "Arguments": str(count)}}]}


def stop_and_answer(index):
    return [control(), {"Kind": "wait_input", "ID": "stop"}, respond(index), {"Kind": "finish"}]


def cases():
    base = [external(), request()]
    yield "plain", {"Actions": base + stop_and_answer(0)}
    yield "completion", {"Actions": base + [respond(response=call()), {"Kind": "add"}, {"Kind": "complete"}, request()] + stop_and_answer(1)}
    yield "two-operations", {"Actions": base + [respond(response=call(count=2)), {"Kind": "add", "Count": 2}, {"Kind": "complete", "Index": 0}, {"Kind": "complete", "Index": 1}, request()] + stop_and_answer(1)}
    yield "hard-model", {"Actions": base + [control("hard"), {"Kind": "wait_input", "ID": "stop"}, {"Kind": "cancelled", "Index": 0}, {"Kind": "finish"}]}
    yield "hard-operation", {"Actions": base + [respond(response=call()), {"Kind": "add"}, control("hard"), {"Kind": "cancel_op"}, {"Kind": "complete", "Status": "canceled"}, {"Kind": "finish"}]}
    yield "steering-stale-reply", {"Actions": base + [external("two", "change course"), request(), {"Kind": "cancelled", "Index": 0}, respond(response=call())] + stop_and_answer(1)}
    yield "after-tools", {"Actions": base + [external("two", "follow up", "after_tools"), respond(response=call()), {"Kind": "add"}, {"Kind": "complete"}, request()] + stop_and_answer(1)}
    yield "steer-deferred", {"Actions": base + [external("two", "follow up", "after_tools"), external("three", "steering"), request(), {"Kind": "cancelled", "Index": 0}] + stop_and_answer(1)}
    yield "stop-deferred", {"Actions": base + [external("two", "follow up", "after_tools"), control("hard"), {"Kind": "wait_input", "ID": "stop"}, {"Kind": "cancelled", "Index": 0}, {"Kind": "finish"}]}
    yield "invalid-immediate-success", {"Actions": base + [respond(response=call("Immediate")), {"Kind": "finish"}]}
    yield "Missing", {"Actions": base + [respond(response=call("Missing")), request()] + stop_and_answer(1)}
    yield "translator-error", {"Actions": base + [respond(response=call("Reject")), {"Kind": "finish"}]}
    yield "failed-input", {"Actions": [{"Kind": "fail", "Fail": "input"}, external(), {"Kind": "finish"}]}
    yield "failed-turn", {"Actions": [{"Kind": "fail", "Fail": "turn"}, external(), {"Kind": "finish"}]}
    for failure in ["response", "status"]:
        yield "failed-" + failure, {"Actions": base + [{"Kind": "fail", "Fail": failure}, respond(response=call()), {"Kind": "finish"}]}
    yield "failed-update", {"Actions": base + [respond(response=call()), {"Kind": "add"}, {"Kind": "fail", "Fail": "update"}, {"Kind": "complete"}, {"Kind": "finish"}]}
    for text in ["<review>only</review>", "<summary></summary>", "<summary>kept task</summary>"]:
        summary = {"Output": [{"Type": "message", "Data": {"Role": "assistant", "Text": text}}]}
        yield "summary-" + text, {"Actions": base + [control("compact", "compact"), {"Kind": "wait_input", "ID": "compact"}, respond(), request(), control(), {"Kind": "wait_input", "ID": "stop"}, respond(1, summary), {"Kind": "finish"}]}
    yield "turn-budget", {"Config": {"MaxTurns": 1}, "Actions": base + [respond(), request(), respond(1), {"Kind": "finish"}]}
    for stop, attempts in [("refused", 2), ("max_output_tokens", 4)]:
        steps = base[:]
        for index in range(attempts):
            response = copy.deepcopy(respond(index)["Response"])
            response["Stop"] = stop
            if index + 1 == attempts:
                steps += [control(), {"Kind": "wait_input", "ID": "stop"}]
            steps += [respond(index, response)]
            if index + 1 < attempts:
                steps += [request()]
        steps += [{"Kind": "finish"}]
        yield stop, {"Actions": steps}
    failed = {"Failure": {"Code": "broken", "Message": "try again"}}
    yield "response-failures", {"Actions": base + [respond(0, failed), request(), respond(1, failed), request(), respond(2, failed), {"Kind": "finish"}]}
    yield "configured-model", {"Model": {"ID": "fixture-model", "ReasoningEffort": "high", "MaxOutputTokens": 1234}, "System": "Owned fixture system", "Actions": base + stop_and_answer(0)}
    yield "duplicate-input", {"Actions": base + [external("one", "must not steer")] + stop_and_answer(0)}
    yield "stopped-stale-reply", {"Actions": base + [control("hard"), {"Kind": "wait_input", "ID": "stop"}, {"Kind": "cancelled"}, respond(response=call()), {"Kind": "finish"}]}
    for status in ["failed", "canceled"]:
        yield "operation-" + status, {"Actions": base + [respond(response=call()), {"Kind": "add"}, {"Kind": "complete", "Status": status}, request()] + stop_and_answer(1)}
    yield "multiple-deferred", {"Actions": base + [external("two", "first deferred", "after_tools"), external("three", "second deferred", "after_tools"), external("four", "steering"), request()] + stop_and_answer(1)}
    yield "explicit-heartbeat", {"Actions": base + [control("heartbeat", "wake"), {"Kind": "wait_input", "ID": "wake"}, respond(), request()] + stop_and_answer(1)}
    settings = {"Kind": "input", "Input": {"ID": "settings", "Kind": "control", "Payload": {"Mode": "settings", "Parameters": {"ReasoningEffort": "high"}}}}
    yield "settings-preserves-inflight", {"Actions": base + [settings, {"Kind": "wait_input", "ID": "settings"}] + stop_and_answer(0)}
    fatal = {"Kind": "respond", "Fault": {"Message": "owned provider failure"}}
    yield "fatal-provider-error", {"Actions": base + [fatal, {"Kind": "finish"}]}
    quota = {"Kind": "respond", "Fault": {"Kind": "quota", "Message": "owned quota", "RetryAfterMS": 1}}
    yield "quota-retry", {"Actions": base + [quota, request()] + stop_and_answer(1)}
    yield "quota-exhausted", {"Actions": base + [quota, request(), {**quota, "Index": 1}, request(), {**quota, "Index": 2}, {"Kind": "finish"}]}
    yield "quota-no-recovery", {"Config": {"NoRecovery": True}, "Actions": base + [quota, {"Kind": "finish"}]}
    cut = copy.deepcopy(respond()["Response"])
    cut["Stop"] = "max_output_tokens"
    yield "cut-no-recovery", {"Config": {"NoRecovery": True}, "Actions": base + [control(), {"Kind": "wait_input", "ID": "stop"}, respond(response=cut), {"Kind": "finish"}]}
    compact = [control("compact", "compact"), {"Kind": "wait_input", "ID": "compact"}, respond(), request()]
    summary = {"Output": [{"Type": "message", "Data": {"Role": "assistant", "Text": "<summary>kept task</summary>"}}]}
    yield "failed-checkpoint", {"Actions": base + compact + [{"Kind": "fail", "Fail": "checkpoint"}, respond(1, summary), {"Kind": "finish"}]}
    yield "input-during-summary", {"Actions": base + compact + [external("two", "work after summary"), {"Kind": "wait_input", "ID": "two"}, respond(1, summary), request()] + stop_and_answer(2)}
    yield "compaction-budget", {"Config": {"MaxCompactions": 1}, "Actions": base + compact + [respond(1, summary), request(), respond(2), {"Kind": "finish"}]}
    overflow = {"Kind": "respond", "Fault": {"Kind": "overflow", "Message": "owned overflow", "Tokens": 20000, "Limit": 10000}}
    yield "overflow-no-recovery", {"Config": {"NoRecovery": True}, "Actions": base + [overflow, {"Kind": "finish"}]}
    for status in ["completed", "failed"]:
        steps = base + [respond(response=call("Write")), {"Kind": "add"}, {"Kind": "complete"}, request(), respond(1), {"Kind": "add", "Count": 2}]
        if status == "completed":
            steps += [control(), {"Kind": "wait_input", "ID": "stop"}, {"Kind": "complete", "Index": 1}, {"Kind": "finish"}]
        else:
            steps += [{"Kind": "complete", "Index": 1, "Status": status}, request()] + stop_and_answer(2)
        yield "verification-" + status, {"Verifier": True, "Actions": steps}
    yield "after-tools-wait-limit", {"Config": {"ToolWaitLimitMS": 5}, "Actions": base + [respond(response=call()), {"Kind": "add"}, external("two", "do not wait forever", "after_tools"), request(), control("hard"), {"Kind": "cancel_op"}, {"Kind": "complete", "Status": "canceled"}, {"Kind": "finish"}]}


def canonicalize(value, spec):
    """Map only introduced identity fields and their quoted diagnostic references."""
    if "OracleError" in value:
        raise AssertionError(value["OracleError"])
    supplied = {step["Input"]["ID"] for step in spec["Actions"] if step["Kind"] == "input"}
    names = {}
    counts = {}

    def introduce(identifier, role):
        if not identifier or identifier in names:
            return
        counts[role] = counts.get(role, 0) + 1
        names[identifier] = f"@{role}:{counts[role]}"

    for event in value["Trace"]:
        kind = event["Kind"]
        payload = event["Value"]
        if kind in ("commit", "write_failure"):
            record_kind = payload["Kind"]
            data = payload["Data"]
            if record_kind == "turn":
                introduce(data["ID"], "turn")
            elif record_kind == "input" and data["ID"] not in supplied:
                introduce(data["ID"], "harness-input")
            elif record_kind in ("tool_call_status", "status"):
                for operation in data.get("Operations", []):
                    introduce(operation["ID"], "operation")
        elif kind in ("dispatch", "update", "cancel"):
            introduce(payload["ID"], "operation")

    def identity(item, key):
        if key in item:
            item[key] = names.get(item[key], item[key])

    output = copy.deepcopy(value)
    for event in output["Trace"]:
        kind = event["Kind"]
        payload = event["Value"]
        if kind in ("commit", "write_failure"):
            record_kind, data = payload["Kind"], payload["Data"]
            if record_kind == "turn":
                identity(data, "ID")
                identity(data, "PreviousTurnID")
            elif record_kind in ("input", "update"):
                identity(data, "ID")
            elif record_kind in ("model_response", "response"):
                identity(data, "TurnID")
            elif record_kind in ("tool_call_status", "status"):
                identity(data, "TurnID")
                identity(data, "CallID")
                status = data["Status"]
                if "WaitingFor" in status:
                    status["WaitingFor"] = [names.get(item, item) for item in status["WaitingFor"]]
                for operation in data.get("Operations", []):
                    identity(operation, "ID")
        elif kind in ("dispatch", "update", "cancel"):
            identity(payload, "ID")
    error = value["Error"]
    for identifier, alias in names.items():
        error = error.replace(json.dumps(identifier), json.dumps(alias))
    output["Error"] = error
    return output


def check_mapping():
    spec = {"Actions": [external()]}
    fixture = {"Trace": [
        {"Kind": "commit", "Value": {"Kind": "turn", "Data": {"ID": "generated-turn"}}},
        {"Kind": "commit", "Value": {"Kind": "input", "Data": {"ID": "one", "Payload": "generated-turn"}}},
        {"Kind": "commit", "Value": {"Kind": "input", "Data": {"ID": "one", "Payload": {"ID": "generated-turn", "generated-turn": {"TurnID": "generated-turn"}}}}},
        {"Kind": "request", "Value": {"Request": {"ID": "generated-turn"}}},
    ], "Error": 'store turn "generated-turn": failure'}
    actual = canonicalize(fixture, spec)
    assert actual["Trace"][0]["Value"]["Data"]["ID"] == "@turn:1"
    assert actual["Trace"][1]["Value"]["Data"] == {"ID": "one", "Payload": "generated-turn"}
    assert actual["Trace"][2:] == fixture["Trace"][2:]
    assert actual["Error"] == 'store turn "@turn:1": failure'
    changed = copy.deepcopy(fixture)
    changed["Error"] += " different diagnostic"
    assert canonicalize(changed, spec) != actual


def invoke(path, spec):
    result = subprocess.run([str(path)], input=json.dumps(spec) + "\n", text=True,
                            capture_output=True, timeout=10, check=True)
    return json.loads(result.stdout)


def main():
    check_mapping()
    directory = ROOT / "tmp/rust-loop-oracle"
    directory.mkdir(parents=True, exist_ok=True)
    go = directory / "go-loop-oracle"
    subprocess.run(["go", "build", "-o", str(go), "./cmd/kou-loop-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-engine", "--example", "loop_oracle"],
                   cwd=ROOT, env={**os.environ, "CARGO_INCREMENTAL": "0"}, check=True)
    rust = ROOT / "tmp/rust-rewrite-target/debug/examples/loop_oracle"
    count = 0
    for name, spec in cases():
        expected = invoke(go, spec)
        actual = invoke(rust, spec)
        try:
            expected = canonicalize(expected, spec)
            actual = canonicalize(actual, spec)
            equal = actual == expected
        except AssertionError as error:
            print("Oracle script failed:", error)
            equal = False
        if not equal:
            print("FAIL:", name)
            output = directory / "failure.json"
            output.write_text(json.dumps({"Case": name, "Spec": spec, "Go": expected, "Rust": actual}, ensure_ascii=False, indent=2))
            print("Full synthetic diff:", output)
            raise SystemExit(1)
        count += 1
    print(f"PASS: {count} actual Go/Rust live coordinator trace scenarios; no tool/provider effects")


if __name__ == "__main__":
    main()
