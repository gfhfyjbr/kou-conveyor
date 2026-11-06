package main

import (
	"os/exec"
	"path/filepath"
	"strings"
	"testing"
)

func TestComposerContextMetrics(t *testing.T) {
	node, err := exec.LookPath("node")
	if err != nil {
		t.Skip("no node")
	}
	file, err := filepath.Abs("plugins/composer/web/composer.js")
	if err != nil {
		t.Fatal(err)
	}
	script := `
import assert from 'node:assert/strict';
const { contextMetrics } = await import(process.argv[1]);

assert.deepEqual(contextMetrics(1_000_000, 50_000), { left: 950_000, share: 0.95 });
assert.deepEqual(contextMetrics(200_000, 50_000), { left: 150_000, share: 0.75 });
assert.deepEqual(contextMetrics(1_000_000, 0), { left: 1_000_000, share: 1 });
assert.deepEqual(contextMetrics(1_000_000, 1_000_000), { left: 0, share: 0 });
assert.deepEqual(contextMetrics(1_000_000, 1_100_000), { left: 0, share: 0 });
for (const window of [0, null, undefined, -1, NaN, Infinity]) {
  assert.equal(contextMetrics(window, 50_000), null, 'unknown window must not be invented');
}
// Both providers and compaction overrides leave physical capacity unchanged.
for (const model of [{ id: 'gpt-6-sol', context: 1_000_000 }, { id: 'claude-opus-5-5', context: 1_000_000 }]) {
  for (const compaction of ['90%', 'off', '']) {
    process.env.KOU_CONVEYOR_AUTO_COMPACT = compaction;
    assert.deepEqual(contextMetrics(model.context, 50_000), { left: 950_000, share: 0.95 }, model.id + '/' + compaction);
  }
}
console.log('ok');
`
	out, err := exec.Command(node, "--input-type=module", "-e", script, "file://"+file).CombinedOutput()
	if err != nil || strings.TrimSpace(string(out)) != "ok" {
		t.Fatalf("node: %v\n%s", err, out)
	}
}
