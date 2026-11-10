#!/usr/bin/env python3
"""ApplyPatch comparisons use disposable fixture trees, never workspace files."""
import json
from pathlib import Path
import random
import shutil
import stat
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "tmp/rust-rewrite-target"


def frame(body):
    return "*** Begin Patch\n" + body + "*** End Patch"


def cases():
    bodies = [
        ("add", "*** Add File: deep/new\n+one\n+two\n"),
        ("add-empty", "*** Add File: new\n"),
        ("add-delete", "*** Add File: new\n+x\n*** Delete File: new\n"),
        ("add-update-move", "*** Add File: new\n+x\n*** Update File: new\n*** Move to: deep/dest\n@@\n-x\n+y\n"),
        ("update", "*** Update File: a\n@@\n-two\n+second\n"),
        ("update-twice", "*** Update File: a\n@@\n-two\n+second\n*** Update File: a\n@@\n-second\n+2\n"),
        ("delete-add", "*** Delete File: a\n*** Add File: a\n+replacement\n"),
        ("move", "*** Update File: a\n*** Move to: deep/dest\n@@\n-two\n+2\n"),
        ("move-only", "*** Update File: a\n*** Move to: renamed\n"),
        ("move-self", "*** Update File: a\n*** Move to: ./a\n@@\n-two\n+second\n"),
        ("move-existing", "*** Update File: a\n*** Move to: crlf\n"),
        ("move-again", "*** Update File: a\n*** Move to: renamed\n*** Update File: renamed\n@@\n-two\n+second\n"),
        ("delete", "*** Delete File: a\n"),
        ("missing", "*** Delete File: absent\n"),
        ("missing-update", "*** Update File: absent\n@@\n-x\n+y\n"),
        ("existing-add", "*** Add File: a\n+x\n"),
        ("directory-delete", "*** Delete File: dir\n"),
        ("directory-update", "*** Update File: dir\n@@\n-x\n+y\n"),
        ("crlf", "*** Update File: crlf\n@@\n-two\n+second\n"),
        ("bare-cr", "*** Update File: bare-cr\n@@\n-one\n+first\n"),
        ("empty-insert", "*** Update File: empty\n@@\n+first\n"),
        ("no-newline", "*** Update File: no-newline\n@@\n-one\n+first\n"),
        ("append-eof", "*** Update File: a\n@@\n+last\n*** End of File\n"),
        ("eof-context", "*** Update File: repeated\n@@\n-x\n+last\n*** End of File\n"),
        ("trim-trailing", "*** Update File: a\n@@\n-two \t\n+second\n"),
        ("trim-surrounding", "*** Update File: fuzzy\n@@\n-old\n+new\n"),
        ("nested", "*** Update File: context\n@@ class Three\n@@ fn four\n-old\n+new\n"),
        ("trim-blank-context", "*** Update File: a\n@@\n-three\n+last\n \n"),
        ("invalid-untouched", "*** Update File: invalid\n@@\n-one\n+first\n"),
        ("symlink", "*** Update File: link\n@@\n-two\n+second\n"),
        ("validation-before-write", "*** Add File: must-not-exist\n+x\n*** Update File: a\n@@\n-absent\n+x\n"),
        ("bad-add", "*** Add File: new\nnot prefixed\n"),
        ("no-path", "*** Add File: \n+x\n"),
        ("unknown-marker", "*** Unexpected: a\n"),
        ("no-op", "*** Update File: a\n"),
        ("context-only", "*** Update File: a\n@@\n one\n"),
        ("bad-header", "*** Update File: a\n@@ not-present\n-one\n+first\n"),
        ("mismatch", "*** Update File: a\n@@\n-no\n+yes\n"),
    ]
    values = [(name, frame(body)) for name, body in bodies]
    values.extend([("no-start", "not a patch"), ("no-end", "*** Begin Patch\n"), ("no-files", frame("")), ("surrounding-text", "note before\n" + frame("*** Update File: a\n@@\n-two\n+2\n") + "\nnote after")])
    rng = random.Random(1190)
    for n in range(80):
        line = rng.randrange(40)
        before = f"line {line}" if n % 7 else "absent"
        if n % 4 == 0: before = " \t" + before + "\t"
        body = f"*** Update File: many\n@@\n-{before}\n+value {n} 中\n"
        if n % 3 == 0: body += "*** Update File: many\n@@\n-line 39\n+last\n"
        values.append((f"generated-{n}", frame(body)))
    return values


def execute(command, directory, patch):
    if directory.exists(): shutil.rmtree(directory)
    directory.mkdir()
    fixtures = {"a":b"one\ntwo\nthree\n", "repeated":b"x\nx\nx\n", "crlf":b"one\r\ntwo\r\n", "bare-cr":b"one\r\ntwo\r\ntail\r", "invalid":b"\xff\xf0\x90\none\n", "empty":b"", "no-newline":b"one", "fuzzy":"\u2003old\t\n".encode(), "context":b"class One\n  fn two\n    old\nclass Three\n  fn four\n    old\n", "many":"".join(f"line {n}\n" for n in range(40)).encode()}
    for name, data in fixtures.items():
        (directory/name).write_bytes(data)
        (directory/name).chmod(0o751 if name == "a" else 0o644)
    (directory/"dir").mkdir(mode=0o755)
    (directory/"link").symlink_to("a")
    result = subprocess.run(list(map(str, command)), input=json.dumps({"Action":"patch","Name":str(directory),"Content":patch}).encode(), stdout=subprocess.PIPE, stderr=subprocess.PIPE, cwd=ROOT, timeout=10, check=True)
    state = {}
    for path in directory.rglob("*"):
        key = str(path.relative_to(directory))
        if path.is_symlink(): state[key] = ("link", str(path.readlink()))
        elif path.is_file(): state[key] = ("file", path.read_bytes(), stat.S_IMODE(path.stat().st_mode))
        else: state[key] = ("directory", stat.S_IMODE(path.stat().st_mode))
    return json.loads(result.stdout), state


def main():
    subprocess.run(["go","build","-o",TARGET/"kou-fileops-oracle","./cmd/kou-fileops-oracle"],cwd=ROOT,check=True)
    subprocess.run(["cargo","build","--locked","-p","kou-tools","--example","fileops_oracle"],cwd=ROOT,check=True)
    corpus = cases()
    with tempfile.TemporaryDirectory(prefix="kou-patch-parity-") as root:
        directory = Path(root)/"fixture"
        for label, patch in corpus:
            left = execute([TARGET/"kou-fileops-oracle"], directory, patch)
            right = execute([TARGET/"debug/examples/fileops_oracle"], directory, patch)
            if left != right: raise AssertionError(f"{label}:\nGo: {left!r}\nRust: {right!r}")
    print(f"PASS: {len(corpus)} ApplyPatch fixtures, exact summaries/errors/files and all resulting bytes/modes/symlinks")


if __name__ == "__main__":
    main()
