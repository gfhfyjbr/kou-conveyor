#!/usr/bin/env python3
"""Trusted corpus: real goja vs QuickJS, with fake plugin capabilities only."""
import json
from pathlib import Path
import subprocess

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "tmp/rust-rewrite-target"

CASES = [
    ("return-value", "console.log('sum', 2+3); return {answer:42};"),
    ("parallel-order", "const r = await Promise.all([echo({n:1}), echo({n:2})]); console.log(r); return r;"),
    ("chaining", "return echo({first:true}).then(value => echo({value}));"),
    ("sleep", "await sleep(1); return 'awake';"),
    ("empty-args", "return await echo();"),
    ("qualified-name", "return await tools['net.fetch']({url:'fake'});"),
    ("collisions", "const a = await tools.JSON({x:1}); const b = await tools.bash({y:2}); return [a,b,typeof JSON.stringify,typeof bash];"),
    ("catch-host-error", "try { await fail({}); } catch(e) { return e.message; }"),
    ("root-host-error", "await fail({});"),
    ("invalid-plugin-array", "await echo([]);"),
    ("invalid-plugin-string", "await echo('x');"),
    ("invalid-read-type", "await read(42);"),
    ("unknown-options", "await read('not-opened', {limti:3});"),
    ("invalid-write-type", "await write('not-written', {});"),
    ("stalled-promise", "await new Promise(() => {});"),
    ("formatting", "console.log(undefined,null,true,3,3n,Symbol('s'),() => 1); console.log(new Map([['a',1]]),new Set([1,2]),Promise.resolve(3)); return {n:NaN,z:-0};"),
    ("captured-json", "JSON.stringify = () => 'wrong'; console.log({x:1}); return {x:2};"),
    ("throw-object", "throw {why:'no'};"),
    ("undefined-return", "console.info('done');"),
    ("symbols", "console.log(Symbol(),Symbol.for('a'),Symbol.iterator);"),
    ("error-names", "try { await fail({}) } catch(e) { return [e.name,e.message] }"),
    ("javascript-error", "throw new Error('plain JavaScript error');"),
    ("call-limit-is-a-rejection", "for(let i=0;i<3;i++) await echo({i}); let p; try {p=echo({})} catch(e) { return 'wrong synchronous throw' } try { await p } catch(e) { return e.message }"),
    ("unicode-bound", "console.log('начало'+'中'.repeat(30000)+'конец'); return '終'.repeat(20000);"),
]


def execute(command, code):
    result = subprocess.run(list(map(str, command)), cwd=ROOT, input=json.dumps({"code": code}).encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10)
    if result.returncode:
        raise RuntimeError("VM oracle failed: " + result.stderr.decode()[:1000])
    return json.loads(result.stdout)


def normalized(value):
    # Only stack source frames and wall-clock timestamps are non-portable.
    # Logs, return text, invocation order, arguments and complete call outcomes
    # are compared exactly; no result body or order normalization is allowed.
    # This is a semantic projection, not a claim that the experimental Rust
    # Execution DTO already has the Go Result wire casing/timestamps.
    if "Calls" in value:
        value = {key[0].lower() + key[1:]: data for key, data in value.items()}
        value["calls"] = [{key[0].lower() + key[1:]: data for key, data in call.items()} for call in value["calls"]]
    assert "calls" in value, "oracle did not return its call vector"
    return {
        "logs": value.get("logs", ""), "value": value.get("value", ""),
        "error": value.get("error", "").split("\n", 1)[0],
        "calls": [{"name": call["name"], "arguments": call["arguments"],
                   "output": call.get("output", ""), "error": call.get("error", ""),
                   "exit_code": call.get("exit_code", call.get("exitCode"))}
                  for call in value.get("calls", [])],
    }


def main():
    go = TARGET / "kou-codevm-oracle"
    subprocess.run(["go", "build", "-o", go, "./cmd/kou-codevm-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-runtime", "--example", "oracle"], cwd=ROOT, check=True)
    rust = TARGET / "debug/examples/oracle"
    for command in ([go], [rust]):
        sanity = normalized(execute(command, "console.log('oracle-sanity'); return 42;"))
        assert sanity["value"] == "42" and sanity["logs"] == "oracle-sanity\n" and sanity["calls"] == [], "oracle no-op/field casing regression"
    failed = []
    for name, code in CASES:
        left = normalized(execute([go], code))
        right = normalized(execute([rust], code))
        if left != right:
            failed.append(name)
            fields = [key for key in left if left[key] != right[key]]
            print(f"FAIL: {name}: {fields}")
            # Only this public, fixed corpus is logged; never user code/data.
            for key in fields:
                if isinstance(left[key], str) and isinstance(right[key], str):
                    difference = next((n for n, (a, b) in enumerate(zip(left[key], right[key])) if a != b), min(len(left[key]), len(right[key])))
                    start = max(0, difference - 40)
                    print(f"  {key}: lengths Go={len(left[key])} Rust={len(right[key])}; first difference at {difference}; Go={left[key][start:difference+80]!r} Rust={right[key][start:difference+80]!r}")
                else:
                    print("  " + key, "Go:", repr(left[key])[:500], "Rust:", repr(right[key])[:500])
        else:
            print("PASS: " + name)
    if failed:
        raise SystemExit(f"{len(failed)} Code VM parity cases failed")
    print(f"{len(CASES)} Code VM cases passed (fake capabilities, no providers)")


if __name__ == "__main__":
    main()
