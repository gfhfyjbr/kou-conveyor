import importlib.util
import json
import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("rust_go_runner", Path(__file__).with_name("run.py"))
runner = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(runner)


class RunnerTests(unittest.TestCase):
    def test_probe_payload_is_required(self):
        with self.assertRaises(RuntimeError):
            runner.payload("testing: warning: no tests to run\nPASS\n")
        self.assertEqual(runner.payload('RUSTGO {"iterations": 2}\nPASS\n'), {"iterations": 2})
        with self.assertRaises(RuntimeError):
            runner.payload('RUSTGO {}\nRUSTGO {}\n')

    def test_environment_resolves_overlay_paths_and_clears_overrides(self):
        with patch.dict(os.environ, {"RUSTGO_MODE": "old", "GOMEMLIMIT": "20MiB", "MALLOC_CONF": "old", "_RJEM_MALLOC_CONF": "old"}):
            env = runner.environment(RUSTGO_MODE="scan")
        self.assertEqual(env["PWD"], str(runner.ROOT))
        self.assertEqual(env["RUSTGO_MODE"], "scan")
        for key in ("GOMEMLIMIT", "MALLOC_CONF", "_RJEM_MALLOC_CONF"):
            self.assertNotIn(key, env)

    def test_fixture_contains_mutable_nested_state_and_unicode(self):
        rows = runner.rich_snapshot(128)
        self.assertEqual(len(rows), 128)
        self.assertIn("Привет", rows[0]["text"])
        self.assertEqual(rows[1]["tool"]["calls"][0]["exit_code"], 0)
        self.assertTrue(any("change" in row.get("tool", {}) for row in rows))
        self.assertTrue(any("images" in row and "files" in row for row in rows))

    def test_private_dump_and_fixture_digest(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "fixture.json"
            runner.dump(path, {"unicode": "🦀"})
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            self.assertEqual(json.loads(path.read_text()), {"unicode": "🦀"})
            info = runner.describe(path, "test")
            self.assertEqual(info["bytes"], path.stat().st_size)
            self.assertEqual(len(info["sha256"]), 64)


if __name__ == "__main__":
    unittest.main()
