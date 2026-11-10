#!/usr/bin/env python3
"""Record tool versions and hashes of the measured baseline/probe source."""
import datetime
import hashlib
import json
import platform
import subprocess
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
HERE = Path(__file__).resolve().parent
sources = sorted(
    list((HERE / "src").glob("*.rs"))
    + list((HERE / "go").glob("*.txt"))
    + list(HERE.glob("*.py"))
    + [HERE / "Cargo.toml", HERE / "Cargo.lock"]
    + [ROOT / path for path in (
        "cmd/internal/cockpit/transcript.go", "cmd/internal/cockpit/sessions.go",
        "harness/primitives/remote_sse.go",
    )]
)
metadata = {
    "timestamp": datetime.datetime.now(datetime.timezone.utc).isoformat(),
    "platform": platform.platform(),
    "go": subprocess.check_output(["go", "version"], text=True).strip(),
    "rust": subprocess.check_output(["rustc", "--version"], text=True).strip(),
    "revision": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip(),
    "source_sha256": {str(path.relative_to(ROOT)): hashlib.sha256(path.read_bytes()).hexdigest() for path in sources},
    "allocator": "tikv-jemallocator 0.7.0, bundled jemalloc 5.3.1; defaults, no MALLOC_CONF",
}
(ROOT / "rust-go-machine.json").write_text(json.dumps(metadata, indent=2) + "\n")
print(f"recorded {len(sources)} source hashes")
