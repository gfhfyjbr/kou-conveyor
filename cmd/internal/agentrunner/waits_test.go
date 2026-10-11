package agentrunner

import (
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
)

func TestAPluginToolThatWaitsSaysSo(t *testing.T) {
	current := plugin.Plugin{Manifest: plugin.Manifest{Name: "waiter"}, Directory: t.TempDir()}
	for _, waits := range []bool{true, false} {
		_, translator, err := pluginTool(current, plugin.Tool{Name: "Wait", Run: []string{"true"}, Waits: waits}, "session", t.TempDir(), t.TempDir())
		if err != nil {
			t.Fatal(err)
		}
		if tool.Waits(translator) != waits {
			t.Fatalf("waits %t: the translator says %t", waits, !waits)
		}
		live := &liveTool{name: "Wait"}
		live.set(translator)
		if live.Waits() != waits {
			t.Fatalf("waits %t: the live tool says %t", waits, !waits)
		}
		// A tool gone answers for the calls made before as it did.
		live.set(nil)
		if live.Waits() != waits {
			t.Fatalf("waits %t: the tool gone says %t", waits, !waits)
		}
	}
}
