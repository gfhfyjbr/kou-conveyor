#!/usr/bin/env python3
"""Build the CSR client and its local JS bindings, without global npm tools."""
import argparse
from pathlib import Path
import shutil
import subprocess

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--debug", action="store_true")
    args = parser.parse_args()
    bindgen = ROOT / "tmp/rust-tools/bin/wasm-bindgen"
    if not bindgen.exists():
        raise SystemExit("Install the local tool: cargo install wasm-bindgen-cli --version 0.2.130 --locked --root tmp/rust-tools")
    command = ["cargo", "build", "--locked", "-p", "kou-web", "--target", "wasm32-unknown-unknown"]
    if not args.debug:
        command.append("--release")
    subprocess.run(command, cwd=ROOT, check=True)
    profile = "debug" if args.debug else "release"
    wasm = ROOT / "tmp/rust-rewrite-target/wasm32-unknown-unknown" / profile / "kou_web.wasm"
    output = ROOT / "tmp/rust-web-assets"
    output.mkdir(parents=True, exist_ok=True)
    subprocess.run([str(bindgen), str(wasm), "--target", "web", "--out-dir", str(output), "--out-name", "kou_web", "--no-typescript"], cwd=ROOT, check=True)
    for name in ("index.html", "boot.js", "style.css"):
        shutil.copyfile(ROOT / "web" / name, output / name)
    sizes = {p.name: p.stat().st_size for p in output.iterdir() if p.is_file()}
    print("Built Rust/WASM assets:", sizes)


if __name__ == "__main__":
    main()
