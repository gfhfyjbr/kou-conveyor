package coordinator

import (
	"bytes"
	"fmt"
	"os"
	"path/filepath"
	"runtime"
	"strconv"
	"testing"
)

// Default: about 8 MiB. Larger runs are opt-in and capped at 512 MiB of
// estimated temporary log data; no existing session directory is opened.
func BenchmarkCheckpointResume(b *testing.B) {
	option := func(name string, fallback int) int {
		if value := os.Getenv(name); value != "" {
			n, err := strconv.Atoi(value)
			if err != nil || n < 1 {
				b.Fatalf("invalid %s=%q", name, value)
			}
			return n
		}
		return fallback
	}
	turns, output := option("KOU_CHECKPOINT_BENCH_TURNS", 128), option("KOU_CHECKPOINT_BENCH_OUTPUT", 64<<10)
	if turns > 4096 || output > 32<<20 || int64(turns)*(int64(output)+2048) > 512<<20 {
		b.Fatal("fixture exceeds the 512 MiB/4096-turn safety cap")
	}
	b.Logf("sample shape: one output=%d bytes; %d turns estimate <=%d bytes plus identity metadata", output, turns, int64(turns)*(int64(output)+2048))
	dir, payload := checkpointBenchmarkFixture(b, turns, output)
	path := filepath.Join(dir, "checkpoint-session.session.jsonl")
	file, err := os.Open(path)
	checkpointRequire(b, err)
	info, err := file.Stat()
	checkpointRequire(b, err)
	tail := make([]byte, min(info.Size(), 2<<20))
	_, err = file.ReadAt(tail, info.Size()-int64(len(tail)))
	checkpointRequire(b, err)
	checkpointRequire(b, file.Close())
	start := bytes.LastIndex(tail, []byte("\n{\"type\":\"checkpoint\",")) + 1
	if start == 0 {
		b.Fatal("checkpoint missing or exceeds measurement tail")
	}
	checkpointBytes := bytes.IndexByte(tail[start:], '\n') + 1
	b.Logf("measured fixture: log=%d bytes; output=%d bytes; latest checkpoint=%d bytes; turns/calls/ops/inputIDs=%d/%d/%d/%d", info.Size(), payload, checkpointBytes, turns+1, turns, turns, turns)
	for _, fast := range []bool{false, true} {
		b.Run(fmt.Sprintf("checkpoint=%v", fast), func(b *testing.B) {
			b.ReportAllocs()
			b.SetBytes(info.Size())
			var retained uint64
			for range b.N {
				b.StopTimer()
				runtime.GC()
				var before, after runtime.MemStats
				runtime.ReadMemStats(&before)
				b.StartTimer()
				current, _ := checkpointRestore(b, dir, fast)
				built, err := current.dependencies.ContextBuilder.Build()
				checkpointRequire(b, err)
				b.StopTimer()
				runtime.GC()
				runtime.ReadMemStats(&after)
				if after.HeapAlloc > before.HeapAlloc {
					retained += after.HeapAlloc - before.HeapAlloc
				}
				runtime.KeepAlive(current)
				runtime.KeepAlive(built)
				b.StartTimer()
			}
			b.ReportMetric(float64(retained)/float64(b.N), "retained-heap-B/op")
			b.ReportMetric(float64(checkpointBytes), "checkpoint-B")
			b.ReportMetric(float64(info.Size()), "log-B")
		})
	}
}
