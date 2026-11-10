#!/usr/bin/env python3
"""Read/Write/Edit differential fixtures; mutates only a private temporary tree."""
import json
from pathlib import Path
import random
import shutil
import stat
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "tmp/rust-rewrite-target"


def cases():
    values = []
    def add(label, data=b"one\ntwo\nthree\n", action="read", **request):
        values.append((label, data, {"Action": action, **request}))
    add("empty", b"")
    add("empty-offset", b"", ReadOptions={"Offset": 50})
    add("numbered")
    add("offset", ReadOptions={"Offset": 2})
    add("limit", ReadOptions={"Offset": 2, "Limit": 1})
    add("byte-budget", ReadOptions={"MaxBytes": 1})
    add("past-eof", ReadOptions={"Offset": 20})
    add("negative-offset", ReadOptions={"Offset": -1})
    add("negative-limit", ReadOptions={"Limit": -1})
    add("binary", b"PNG\0", Suffix=".PNG")
    add("directory", None, Directory=True)
    add("missing", None)
    add("directory-quotes", None, Directory=True, Suffix=" 'directory")
    for n in range(1, 9):
        add(f"unicode-cut-{n}", "a中終🎉z\r\nend\r".encode(), ReadOptions={"MaxLineBytes": n})
    add("invalid-utf8-display", b"\xff\xf0\x90\na\xed\xa0\x80\n")
    add("giant-line", b"x" * (2 << 20) + b"\r\nlast\n", ReadOptions={"MaxLineBytes": 7})
    add("nul-after-head", b"x" * 8192 + b"\0\nlast\n", ReadOptions={"MaxLineBytes": 8})
    add("delete-entire", b"x", "edit", Old="x", Replacement="")
    add("same", action="edit", Old="x", Replacement="x")
    add("empty-old", action="edit", Old="", Replacement="x")
    add("ambiguous", b"aaaa\n", "edit", Old="aa", Replacement="b")
    add("nonoverlapping", b"aaaa\n", "edit", Old="aa", Replacement="b", EditOptions={"ReplaceAll": True})
    add("whitespace-hint", b"if x {\n    return\n}\n", "edit", Old="if x {\n\treturn\n}", Replacement="z")
    add("unicode-whitespace", "a\u0085b\u2003c\n".encode(), "edit", Old="abc", Replacement="z")
    add("first-line-hint", b"if x {\n    return\n}\n", "edit", Old="if x {\n\treturn 5\n}", Replacement="z")
    add("not-found", action="edit", Old="absent", Replacement="x")
    add("crlf", b"one\r\ntwo\r\n", "edit", Old="one\ntwo\n", Replacement="three\n", Mode=0o751)
    add("invalid-untouched", b"\xff\xf0\x90\none\nend\n", "edit", Old="one", Replacement="two")
    add("symlink-edit", action="edit", Old="two", Replacement="second", Symlink=True)
    add("symlink-write", action="write", Content="new\n", Symlink=True)
    add("create", None, "write", Content="one\ntwo\n", Deep=True)
    add("replace", action="write", Content="three", Mode=0o755)
    add("empty-write", action="write", Content="")
    add("binary-edit", b"a\0b", "edit", Old="a", Replacement="c")
    add("directory-edit", None, "edit", Directory=True, Old="x", Replacement="z")
    add("directory-write", None, "write", Directory=True, Content="x")
    rng = random.Random(9129)
    for n in range(80):
        text = "\n".join("".join(rng.choices("ab中終\t \r", k=rng.randrange(0, 25))) for _ in range(rng.randrange(1, 18)))
        if rng.randrange(2): text += "\n"
        add(f"random-read-{n}", text.encode(), ReadOptions={"Offset": rng.randrange(0, 22), "Limit": rng.choice([0, 1, 4, 30]), "MaxLineBytes": rng.randrange(1, 15), "MaxBytes": rng.choice([1, 20, 100, 0])})
    for n in range(60):
        text = "\n".join(f"line {i} value {rng.randrange(8)}" for i in range(30)) + "\n"
        old = f"value {rng.randrange(10)}"
        add(f"random-edit-{n}", text.encode(), "edit", Old=old, Replacement="中\nnew" if n % 4 == 0 else "changed", EditOptions={"ReplaceAll": bool(n % 2)})
    return values


def prepare(directory, data, request):
    # Reuse the exact same path for both implementations so errors compare
    # verbatim. Delete only the fixture tree owned by TemporaryDirectory.
    if directory.exists(): shutil.rmtree(directory)
    directory.mkdir()
    suffix = request.get("Suffix", "")
    path = directory / ("deep/dir/file" if request.get("Deep") else "file")
    path = path.with_name(path.name + suffix)
    if request.get("Directory"):
        path.mkdir()
    elif data is not None:
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        path.chmod(request.get("Mode", 0o644))
    if request.get("Symlink"):
        link = directory / "link"
        link.symlink_to(path.name)
        return link, path
    return path, path


def execute(command, directory, data, request):
    path, target = prepare(directory, data, request)
    payload = {key: value for key, value in request.items() if key in ("Action", "Content", "Old", "Replacement", "ReadOptions", "EditOptions")}
    payload["Name"] = str(path)
    completed = subprocess.run(list(map(str, command)), input=json.dumps(payload).encode(), cwd=ROOT, stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=10, check=True)
    result = json.loads(completed.stdout)
    assert set(result) == {"Result", "Error"}, "oracle schema/no-op regression"
    state = {"link": path.is_symlink(), "exists": target.exists()}
    if target.is_file():
        state.update(data=target.read_bytes(), mode=stat.S_IMODE(target.stat().st_mode))
    return result, state


def main():
    subprocess.run(["go", "build", "-o", TARGET / "kou-fileops-oracle", "./cmd/kou-fileops-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-tools", "--example", "fileops_oracle"], cwd=ROOT, check=True)
    corpus = cases()
    with tempfile.TemporaryDirectory(prefix="kou-fileops-parity-") as root:
        directory = Path(root) / "fixture"
        for label, data, request in corpus:
            left = execute([TARGET / "kou-fileops-oracle"], directory, data, request)
            right = execute([TARGET / "debug/examples/fileops_oracle"], directory, data, request)
            if left != right:
                raise AssertionError(f"{label}:\nGo: {left!r}\nRust: {right!r}")
    print(f"PASS: {len(corpus)} Read/Write/Edit fixtures, exact results/errors and final bytes/modes/symlinks")


if __name__ == "__main__":
    main()
