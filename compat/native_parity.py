#!/usr/bin/env python3
"""Trusted real-capability corpus. Only this private fixture directory is writable."""
import json
from pathlib import Path
import shutil
import stat
import subprocess
import tempfile
from codevm_parity import normalized

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "tmp/rust-rewrite-target"
CASES = [
    ("file-roundtrip", "await write('new.txt','one\\ntwo\\n'); await edit('new.txt','two','second'); console.log(await read('new.txt',{offset:2})); return await readText('new.txt');"),
    ("read-offset-limit", "return await read('original',{offset:2,limit:1});"),
    ("read-crlf", "return await read('crlf');"),
    ("read-utf8", "return await read('unicode');"),
    ("read-missing", "try {return await read('missing')} catch(e) {return e.message}"),
    ("read-text-missing", "try {return await readText('missing')} catch(e) {return e.message}"),
    ("binary-read", "try {return await read('binary')} catch(e) {return e.message}"),
    ("crlf-edit", "return await edit('crlf','one\\ntwo\\n','three\\n');"),
    ("symlink-edit", "await edit('link','two','second'); return await readText('original');"),
    ("ambiguous", "try {await edit('repeated','x','z')} catch(e) {return e.message}"),
    ("replace-all", "return await edit('repeated','x','z',{replaceAll:true});"),
    ("coerce-replace-all", "return await edit('repeated','x','z',{replaceAll:'yes'});"),
    ("coerce-numeric-read", "return await read('original',{offset:'2',limit:true});"),
    ("empty-old", "try {await edit('original','','z')} catch(e) {return e.message}"),
    ("write-empty", "return await write('original','');"),
    ("shell-streams", "return await bash(\"printf 'out\\n'; printf 'err\\n' >&2; exit 7\");"),
    ("shell-empty-env", "return await bash(\"printf '%s' \" + '\"$HOME\"');"),
    ("shell-bounded", "return (await bash('printf 123456789012345678901234567890',{maxOutputLength:20})).stdout;"),
    ("shell-invalid-utf8", r"return (await bash(\"printf 'a\\377\\377b'\")).stdout;".replace(r'\"', '"')),
    ("plugin-echo", "return await echo({answer:42,text:'a\\nb'});"),
    ("plugin-probe-call-id", "await sleep(1); const a=await probe({n:1}); await bash('true'); return [a,await probe({n:2})];"),
    ("plugin-error", "try {await fail({})} catch(e) {return e.message}"),
    ("plugin-parallel", "return await Promise.all([echo({n:1}),echo({n:2}),echo({n:3})]);"),
    ("patch-update", "return await applyPatch('*** Begin Patch\\n*** Update File: original\\n@@\\n-two\\n+second\\n*** End Patch');"),
    ("patch-error", "try {await applyPatch('*** Begin Patch\\n*** Update File: original\\n@@\\n-no\\n+yes\\n*** End Patch')} catch(e) {return e.message}"),
    ("skill", "return await skill('example');"),
    ("missing-skill", "try {await skill('missing')} catch(e) {return e.message}"),
    ("view-unavailable", "try {await viewImage('original')} catch(e) {return e.message}"),
    ("search-unavailable", "try {await transcriptSearch('needle')} catch(e) {return e.message}"),
]


def execute(command, directory, code):
    if directory.exists(): shutil.rmtree(directory)
    directory.mkdir()
    fixtures = {"original":b"one\ntwo\nthree\n", "repeated":b"x x\n", "crlf":b"one\r\ntwo\r\n", "unicode":"中終🎉\n".encode(), "binary":b"a\0b", "SKILL.md":b"# Example\nDo the exact thing.\n"}
    for name, data in fixtures.items():
        (directory/name).write_bytes(data)
        (directory/name).chmod(0o644)
    (directory/"link").symlink_to("original")
    result = subprocess.run(list(map(str, command)), input=json.dumps({"Code":code,"Directory":str(directory)}).encode(), cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10, check=True)
    state = {str(path.relative_to(directory)): ("link", str(path.readlink())) if path.is_symlink() else ("file", path.read_bytes(), stat.S_IMODE(path.stat().st_mode)) for path in directory.iterdir() if path.is_symlink() or path.is_file()}
    return normalized(json.loads(result.stdout)), state


def main():
    subprocess.run(["go","build","-o",TARGET/"kou-native-oracle","./cmd/kou-native-oracle"],cwd=ROOT,check=True)
    subprocess.run(["cargo","build","--locked","-p","kou-runtime","--example","native_oracle"],cwd=ROOT,check=True)
    failed = []
    with tempfile.TemporaryDirectory(prefix="kou-native-parity-") as root:
        directory = Path(root)/"fixture"
        for label, code in CASES:
            left = execute([TARGET/"kou-native-oracle"], directory, code)
            right = execute([TARGET/"debug/examples/native_oracle"], directory, code)
            if left != right:
                failed.append(label)
                print(f"FAIL {label}:\nGo: {left!r}\nRust: {right!r}")
            else:
                print(f"PASS: {label}")
    if failed: raise SystemExit(f"{len(failed)} native differences: {', '.join(failed)}")
    print(f"PASS: {len(CASES)} native VM cases, exact values/logs/call outcomes and file states; no providers")


if __name__ == "__main__":
    main()
