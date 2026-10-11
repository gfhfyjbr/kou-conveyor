package anthropic

import (
	"encoding/json/v2"
	"slices"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// prefixFor fingerprints the request params build for input.
func prefixFor(t *testing.T, client *Client, input []llm.Item, tools ...llm.Tool) llm.Prefix {
	t.Helper()
	params, err := client.params(llm.Request{Model: llm.Model{ID: "claude-opus-5", ReasoningEffort: llm.ReasoningEffortHigh}, Input: input, Tools: tools}, true, 0)
	if err != nil {
		t.Fatal(err)
	}
	before, err := json.Marshal(params)
	if err != nil {
		t.Fatal(err)
	}
	prefix := prefixOf(params)
	after, err := json.Marshal(params)
	if err != nil {
		t.Fatal(err)
	}
	if string(before) != string(after) {
		t.Fatalf("fingerprinting changed the request:\n%s\n%s", before, after)
	}
	return prefix
}

// same counts the messages two prefixes begin with alike.
func same(a, b llm.Prefix) int {
	n := 0
	for n < len(a.Messages) && n < len(b.Messages) && a.Messages[n] == b.Messages[n] {
		n++
	}
	return n
}

// The fingerprints say where a request stops being the one before: a turn
// only adds to it, though the cache markers move; a message rewritten
// shows at its index; a new system prompt or tool shows on its own.
func TestPrefixShowsWhereARequestChanged(t *testing.T) {
	client, err := NewClient(Config{APIKey: "k"})
	if err != nil {
		t.Fatal(err)
	}
	system := item(llm.ItemMessage, llm.Message{Role: llm.RoleSystem, Text: "You are careful."})
	user := func(text string) llm.Item { return item(llm.ItemMessage, llm.Message{Role: llm.RoleUser, Text: text}) }
	answer := func(text string) llm.Item {
		return item(llm.ItemMessage, llm.Message{Role: llm.RoleAssistant, Text: text})
	}
	tool := conversationRequest("claude-opus-5").Tools[0]
	turn1 := []llm.Item{system, user("one"), answer("1"), user("two")}
	turn2 := append(slices.Clone(turn1), answer("2"), user("three"))
	first, next := prefixFor(t, client, turn1, tool), prefixFor(t, client, turn2, tool)

	if len(first.Messages) != 3 || len(next.Messages) != 5 || first.Tools == "" || len(first.System) != 1 {
		t.Fatalf("prefix = %+v", first)
	}
	// The markers moved from messages 0 and 2 to 2 and 4; the messages did not.
	if got := same(first, next); got != 3 || first.Tools != next.Tools || !slices.Equal(first.System, next.System) {
		t.Fatalf("a turn changed the prefix: same %d of 3", got)
	}
	want := []llm.Breakpoint{{Message: -1, TTL: "5m"}, {Message: 0, TTL: "5m"}, {Message: 2, TTL: "5m"}}
	if !slices.Equal(first.Breakpoints, want) {
		t.Fatalf("breakpoints = %+v, want %+v", first.Breakpoints, want)
	}
	if string(first.Edits) != `[{"keep":{"value":2,"type":"thinking_turns"},"type":"clear_thinking_20251015"}]` {
		t.Fatalf("edits = %s", first.Edits)
	}

	// A message rewritten — the summary that starts a resumed run, a tool
	// result cut short — changes the prefix from its index on.
	rewritten := slices.Clone(turn2)
	rewritten[3] = user("two, said again")
	if got := same(next, prefixFor(t, client, rewritten, tool)); got != 2 {
		t.Fatalf("rewritten message 2: same %d", got)
	}
	rewritten = slices.Clone(turn2)
	rewritten[1] = user("one, summarized")
	if got := same(next, prefixFor(t, client, rewritten, tool)); got != 0 {
		t.Fatalf("rewritten message 0: same %d", got)
	}

	// A system prompt or the tools change on their own.
	other := slices.Clone(turn2)
	other[0] = item(llm.ItemMessage, llm.Message{Role: llm.RoleSystem, Text: "You are quick."})
	changed := prefixFor(t, client, other, tool)
	if slices.Equal(changed.System, next.System) || changed.Tools != next.Tools || same(changed, next) != 5 {
		t.Fatalf("a new system prompt: %+v", changed)
	}
	tool.Description = "Run a shell command."
	changed = prefixFor(t, client, turn2, tool)
	if changed.Tools == next.Tools || !slices.Equal(changed.System, next.System) || same(changed, next) != 5 {
		t.Fatalf("a tool changed: %+v", changed)
	}
}

// Respond reports the prefix of the request a response answered, with the
// response, only to a context that asks; a request that failed and was
// sent again is reported once.
func TestRespondReportsThePrefix(t *testing.T) {
	s := newServer(t, apiError(529, "overloaded_error", "busy"), textResponse("ok"), textResponse("again"))
	client := newTestClient(t, s, Config{})
	var reports []llm.PrefixReport
	ctx := llm.WithPrefixReporter(t.Context(), func(report llm.PrefixReport) { reports = append(reports, report) })
	if _, err := client.Respond(ctx, conversationRequest("claude-opus-5"), llm.RequestOptions{}); err != nil {
		t.Fatal(err)
	}
	if len(reports) != 1 {
		t.Fatalf("%d reports", len(reports))
	}
	report := reports[0]
	if report.Response != "msg_1" || report.Model != "claude-opus-5" || report.Usage.CachedInputTokens != 100 ||
		report.Usage.CacheWriteInputTokens != 5 || len(report.Prefix.Messages) != 1 || len(report.Prefix.System) != 1 {
		t.Fatalf("report = %+v", report)
	}
	if _, err := client.Respond(t.Context(), conversationRequest("claude-opus-5"), llm.RequestOptions{}); err != nil {
		t.Fatal(err)
	}
	if len(reports) != 1 {
		t.Fatal("a context without a reporter was reported to")
	}
}
