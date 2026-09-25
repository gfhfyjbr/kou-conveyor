package contextbuilder

import (
	"encoding/json/v2"
	"fmt"
	"reflect"
	"strings"
	"testing"

	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

func addInput(t *testing.T, current Builder, text string) {
	t.Helper()
	payload, err := json.Marshal(text)
	if err != nil {
		t.Fatal(err)
	}
	if err := current.AddExternalInput(inbox.Input{ID: inbox.ID(text), Kind: inbox.InputExternal, Payload: payload}); err != nil {
		t.Fatal(err)
	}
}

func answer(text string, calls ...llm.ToolCall) llm.Response {
	response := llm.Response{Output: []llm.Item{{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: text}}}}
	for _, call := range calls {
		response.Output = append(response.Output, llm.Item{Type: llm.ItemToolCall, Data: call})
	}
	return response
}

func TestBuilderBuildsCompactionRequest(t *testing.T) {
	current := NewBuilder()
	current.AddTool(llm.Tool{Type: llm.ToolFunction, Name: "Bash"})
	addInput(t, current, "fix the build")
	current.Commit()
	current.AddModelResponse(answer("Looking."))
	addInput(t, current, "and the tests")

	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	compaction, err := current.BuildCompaction("  the failing tests  ", 0)
	if err != nil {
		t.Fatal(err)
	}
	input := compaction.Request.Input
	// The compaction request is the ordinary one with the instructions after
	// it, so the provider's prompt cache still covers the conversation.
	if !reflect.DeepEqual(input[:len(input)-1], built.Request.Input) || !reflect.DeepEqual(compaction.Request.Tools, built.Request.Tools) {
		t.Fatal("compaction request does not extend the ordinary request")
	}
	prompt := input[len(input)-1].Data.(llm.Message)
	// The focus comes before the closing reminder.
	if prompt.Role != llm.RoleUser || !strings.HasPrefix(prompt.Text, compactionPrompt) ||
		!strings.HasSuffix(prompt.Text, "The user asked the summary to focus on:\nthe failing tests\n\n"+compactionReminder) {
		t.Fatalf("compaction prompt = %#v", prompt)
	}
	if want := estimateItem(userMessage("and the tests")); built.PendingTokens != want {
		t.Fatalf("pending tokens = %d, want %d", built.PendingTokens, want)
	}
	if compaction.EstimatedTokens <= built.EstimatedTokens || !compaction.Compactable {
		t.Fatalf("estimate %d after %d, compactable %v", compaction.EstimatedTokens, built.EstimatedTokens, compaction.Compactable)
	}
	unfocused, err := current.BuildCompaction("", 0)
	if err != nil {
		t.Fatal(err)
	}
	if messageText(unfocused.Request.Input[len(unfocused.Request.Input)-1]) != compactionPrompt+"\n\n"+compactionReminder {
		t.Fatal("an empty focus changed the compaction prompt")
	}
}

func TestBuilderCompactsConversation(t *testing.T) {
	current := NewBuilder()
	current.SetSystemPrompt("be brief")
	addInput(t, current, "first task")
	current.Commit()
	running := llm.ToolCall{CallID: "call-running", Name: "Bash", Arguments: "{\n  \"command\": \"make test\"\n}"}
	finished := llm.ToolCall{CallID: "call-finished", Name: "Bash", Arguments: `{"command":"ls"}`}
	late := llm.ToolCall{CallID: "call-late", Name: "ViewImage", Arguments: `{"path":"shot.png"}`}
	current.AddModelResponse(answer("Working on it.", running, finished, late))
	current.AddToolResult("call-running", nil, true)
	current.AddToolResult("call-finished", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: "file.txt"}}, false)
	current.AddToolResult("call-late", nil, true)
	addInput(t, current, "second task")
	current.Commit() // the compaction turn

	// Arrived while the compaction ran.
	addInput(t, current, "third task")
	current.AddToolResult("call-late", []llm.ToolResultOutput{{Kind: llm.ToolResultImage, Value: "data:image/png;base64,AAAA"}}, false)

	if current.Compact(llm.Response{Output: []llm.Item{{Type: llm.ItemReasoning, Data: llm.Reasoning{Summary: []string{"thinking"}}}}}) {
		t.Fatal("compacted without a summary")
	}
	if !current.Compact(answer("  The summary.  ")) {
		t.Fatal("did not compact")
	}
	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	input := built.Request.Input
	if len(input) != 5 || input[0].Data.(llm.Message).Role != llm.RoleSystem || !strings.HasSuffix(messageText(input[0]), "be brief") {
		t.Fatalf("compacted input = %#v", input)
	}
	summary := messageText(input[1])
	for _, want := range []string{
		compactedPreface,
		"<user_messages>\n<message>\nfirst task\n</message>\n</user_messages>",
		`- call-running (Bash { "command": "make test" })`,
		"<summary>\nThe summary.\n</summary>",
	} {
		if !strings.Contains(summary, want) {
			t.Fatalf("summary message lacks %q:\n%s", want, summary)
		}
	}
	// The finished call and the one whose result came in during the compaction
	// are not running; the pending prompt is not an earlier one.
	for _, unwanted := range []string{"call-finished", "call-late", "second task"} {
		if strings.Contains(summary, unwanted) {
			t.Fatalf("summary message names %q:\n%s", unwanted, summary)
		}
	}
	if messageText(input[2]) != "second task" || messageText(input[3]) != "third task" {
		t.Fatalf("pending prompts = %q, %q", messageText(input[2]), messageText(input[3]))
	}
	if got := messageText(input[4]); got != "Result of the tool call call-late (ViewImage {\"path\":\"shot.png\"}), made before the conversation was compacted:\n"+
		"[An image was attached here. It cannot be shown without its call; view it again if it is still needed.]" {
		t.Fatalf("detached result = %q", got)
	}
	if built.Compactable || len(built.Report.Changes) != 1 || built.Report.Changes[0].Kind != ChangeCompacted {
		t.Fatalf("compactable %v, report %#v", built.Compactable, built.Report)
	}

	// The running call finishes later: its result has no call to answer.
	current.Commit()
	current.AddToolResult("call-running", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: "ok"}}, false)
	built, err = current.Build()
	if err != nil {
		t.Fatal(err)
	}
	last := built.Request.Input[len(built.Request.Input)-1]
	if last.Type != llm.ItemMessage || messageText(last) != "Result of the tool call call-running (Bash { \"command\": \"make test\" }), made before the conversation was compacted:\nok" {
		t.Fatalf("late result = %#v", last)
	}
	for _, item := range built.Request.Input {
		if item.Type == llm.ItemToolResult || item.Type == llm.ItemToolCall {
			t.Fatalf("compacted context kept %#v", item)
		}
	}
}

func TestBuilderDescribesRunningResultAfterCompaction(t *testing.T) {
	current := NewBuilder()
	addInput(t, current, "task")
	current.Commit()
	current.AddModelResponse(answer("", llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{"command":"sleep 60"}`}))
	current.Commit()
	if !current.Compact(answer("Summary.")) {
		t.Fatal("did not compact")
	}
	current.AddToolResult("call-1", nil, true)
	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	if got := messageText(built.Request.Input[len(built.Request.Input)-1]); got != `The tool call call-1 (Bash {"command":"sleep 60"}), made before the conversation was compacted, is still running; its result arrives in a later turn.` {
		t.Fatalf("running placeholder = %q", got)
	}
}

func TestBuilderKeepsNewestPromptsThatFit(t *testing.T) {
	current := NewBuilder()
	long := strings.Repeat("x", keptInputLimit+500) + "tail"
	prompts := []string{"the very first prompt"}
	for index := range 6 {
		prompts = append(prompts, fmt.Sprintf("prompt %d %s", index, strings.Repeat("y", 7000)))
	}
	prompts = append(prompts, long)
	for _, prompt := range prompts {
		addInput(t, current, prompt)
		current.Commit()
		current.AddModelResponse(answer("ok"))
	}
	current.Commit()
	if !current.Compact(answer("Summary.")) {
		t.Fatal("did not compact")
	}
	built, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	summary := messageText(built.Request.Input[1])
	if strings.Contains(summary, "the very first prompt") || strings.Contains(summary, "prompt 0 ") || !strings.Contains(summary, "prompt 5 ") ||
		!strings.Contains(summary, "earlier ones are left out") || !strings.Contains(summary, "characters left out …]\n") || !strings.Contains(summary, "tail\n</message>") {
		t.Fatalf("kept prompts:\n%.600s", summary)
	}
	if len(summary) > keptInputsBudget+8000 {
		t.Fatalf("summary message is %d bytes", len(summary))
	}
}

func TestBuilderEstimatesRequestSize(t *testing.T) {
	current := NewBuilder()
	empty, err := current.Build()
	if err != nil {
		t.Fatal(err)
	}
	if empty.EstimatedTokens < estimateText(preamble) || empty.Compactable {
		t.Fatalf("empty estimate %d, compactable %v", empty.EstimatedTokens, empty.Compactable)
	}
	current.AddTool(llm.Tool{Type: llm.ToolFunction, Name: "Bash", Description: strings.Repeat("d", 400)})
	withTool, _ := current.Build()
	if withTool.EstimatedTokens < empty.EstimatedTokens+100 {
		t.Fatalf("tool estimate %d after %d", withTool.EstimatedTokens, empty.EstimatedTokens)
	}

	addInput(t, current, "task")
	current.Commit()
	response := answer("reply", llm.ToolCall{CallID: "call-1", Name: "Bash", Arguments: `{}`})
	response.Usage = llm.Usage{InputTokens: 90_000, CachedInputTokens: 80_000, OutputTokens: 10_000, ReasoningTokens: 5_000}
	current.AddModelResponse(response)
	reported, _ := current.Build()
	if reported.EstimatedTokens != 100_000 {
		t.Fatalf("estimate right after a response = %d, want the reported 100000", reported.EstimatedTokens)
	}
	// Everything after the response is estimated from its size.
	current.AddToolResult("call-1", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: strings.Repeat("z", 4000)}, {Kind: llm.ToolResultImage, Value: "data:image/png;base64," + strings.Repeat("A", 100_000)}}, false)
	grown, _ := current.Build()
	if want := int64(100_000 + itemTokens + 1000 + imageTokens); grown.EstimatedTokens != want {
		t.Fatalf("estimate with a result = %d, want %d", grown.EstimatedTokens, want)
	}
	current.Commit()
	if !current.Compact(answer("Summary.")) {
		t.Fatal("did not compact")
	}
	compacted, _ := current.Build()
	if compacted.EstimatedTokens >= 5000 || compacted.EstimatedTokens <= withTool.EstimatedTokens {
		t.Fatalf("estimate after compaction = %d", compacted.EstimatedTokens)
	}
}

func TestBuilderFitsCompactionRequestToBudget(t *testing.T) {
	current := NewBuilder()
	addInput(t, current, "inspect the logs")
	current.Commit()
	current.AddModelResponse(answer("", llm.ToolCall{CallID: "small", Name: "Bash", Arguments: `{}`}, llm.ToolCall{CallID: "large", Name: "Bash", Arguments: `{}`}))
	current.AddToolResult("small", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: strings.Repeat("s", 3000)}}, false)
	current.AddToolResult("large", []llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: strings.Repeat("l", 200_000)}}, false)
	whole, err := current.BuildCompaction("", 0)
	if err != nil {
		t.Fatal(err)
	}

	// Cutting the largest result is enough.
	prompt := itemTokens + estimateText(compactionPrompt+"\n\n"+compactionReminder)
	fitted, err := current.BuildCompaction("", prompt+20_000)
	if err != nil {
		t.Fatal(err)
	}
	if fitted.EstimatedTokens > prompt+20_000 || len(fitted.Request.Input) != len(whole.Request.Input) {
		t.Fatalf("fitted to %d tokens in %d items", fitted.EstimatedTokens, len(fitted.Request.Input))
	}
	results := map[string]int{}
	for _, item := range fitted.Request.Input {
		if result, ok := item.Data.(llm.ToolResult); ok {
			results[result.CallID] = len(result.Output[0].Value)
		}
	}
	if results["small"] != 3000 || results["large"] > trimmedResultBytes+100 {
		t.Fatalf("result sizes = %v", results)
	}
	if changes := fitted.Report.Changes; len(changes) != 1 || changes[0].Kind != ChangeTruncated {
		t.Fatalf("report = %#v", fitted.Report)
	}
	// The conversation itself keeps every result whole.
	again, _ := current.BuildCompaction("", 0)
	if !reflect.DeepEqual(again.Request, whole.Request) {
		t.Fatal("fitting a compaction request changed the conversation")
	}

	// A budget smaller still leaves the oldest items out, and a result that
	// loses its call reads as a message.
	tight, err := current.BuildCompaction("", prompt+1200)
	if err != nil {
		t.Fatal(err)
	}
	input := tight.Request.Input
	if tight.EstimatedTokens > prompt+1200 || !strings.HasPrefix(messageText(input[1]), "[") || !strings.Contains(messageText(input[1]), "earlier items of the conversation are left out") {
		t.Fatalf("tight input = %#v", input)
	}
	for _, item := range input {
		if item.Type == llm.ItemToolResult {
			t.Fatalf("a result kept without its call: %#v", item)
		}
	}
	if !strings.HasPrefix(messageText(input[len(input)-2]), "Result of the tool call large (Bash {})") || !strings.HasPrefix(messageText(input[len(input)-1]), compactionPrompt) {
		t.Fatalf("tight input ends with %#v", input[len(input)-2:])
	}
	if changes := tight.Report.Changes; len(changes) != 2 || changes[1].Kind != ChangeOmitted {
		t.Fatalf("report = %#v", tight.Report)
	}
}

func TestBuilderFitKeepsTheEarlierSummary(t *testing.T) {
	current := NewBuilder()
	addInput(t, current, "task")
	current.Commit()
	current.AddModelResponse(answer("First part done."))
	current.Commit()
	if !current.Compact(answer("The earlier summary.")) {
		t.Fatal("did not compact")
	}
	for index := range 20 {
		addInput(t, current, fmt.Sprintf("step %d %s", index, strings.Repeat("x", 4000)))
		current.Commit()
		current.AddModelResponse(answer("ok"))
	}
	prompt := itemTokens + estimateText(compactionPrompt+"\n\n"+compactionReminder)
	fitted, err := current.BuildCompaction("", prompt+5000)
	if err != nil {
		t.Fatal(err)
	}
	input := fitted.Request.Input
	if !strings.Contains(messageText(input[1]), "<summary>\nThe earlier summary.\n</summary>") || !strings.Contains(messageText(input[2]), "left out") ||
		messageText(input[len(input)-2]) != "ok" {
		t.Fatalf("fitted input = %#v", input)
	}
	if fitted.EstimatedTokens > prompt+5000 {
		t.Fatalf("estimate %d", fitted.EstimatedTokens)
	}
}

func TestSummaryKeepsTheSummaryBlock(t *testing.T) {
	message := func(texts ...string) llm.Response {
		var response llm.Response
		for _, text := range texts {
			response.Output = append(response.Output, llm.Item{Type: llm.ItemMessage, Data: llm.Message{Role: llm.RoleAssistant, Text: text}})
		}
		return response
	}
	for name, test := range map[string]struct {
		response llm.Response
		want     string
	}{
		"analysis and summary":    {message("<analysis>\nthinking it over\n</analysis>\n\n<summary>\n1. Primary request: ship it\n</summary>"), "1. Primary request: ship it"},
		"summary only":            {message("<summary>The gist.</summary>"), "The gist."},
		"no blocks":               {message("  Plain summary.  "), "Plain summary."},
		"analysis only":           {message("<analysis>notes</analysis>\nWhat is left."), "What is left."},
		"split across messages":   {message("<analysis>notes</analysis>", "<summary>\nFrom the second part.\n</summary>"), "From the second part."},
		"nothing":                 {message("<analysis>only notes</analysis>"), ""},
		"cut off in the summary":  {message("<analysis>notes</analysis>\n<summary>\n1. Primary request: ship it\n2. Key"), "1. Primary request: ship it\n2. Key"},
		"cut off in the analysis": {message("Here it is.\n<analysis>\nFirst the user asked"), ""},
		"unclosed analysis":       {message("<analysis>notes\n<summary>The gist.</summary>"), "The gist."},
	} {
		if got := Summary(test.response); got != test.want {
			t.Errorf("%s: summary = %q, want %q", name, got, test.want)
		}
	}
}

func TestCompactedMessagePointsToTheTranscript(t *testing.T) {
	for _, transcript := range []string{"", "/work/.harness/sessions/abc.session.jsonl"} {
		current := NewBuilder()
		current.SetTranscript(transcript)
		addInput(t, current, "task")
		current.Commit()
		current.AddModelResponse(answer("done"))
		current.Commit()
		if !current.Compact(answer("<analysis>notes</analysis><summary>The work so far.</summary>")) {
			t.Fatal("did not compact")
		}
		built, err := current.Build()
		if err != nil {
			t.Fatal(err)
		}
		text := messageText(built.Request.Input[1])
		if !strings.Contains(text, "<summary>\nThe work so far.\n</summary>") || strings.Contains(text, "notes") || !strings.HasSuffix(text, compactedResume) {
			t.Fatalf("compacted message:\n%s", text)
		}
		if mentions := strings.Contains(text, "full transcript at "+transcript+" "); mentions != (transcript != "") {
			t.Fatalf("transcript %q mentioned: %v\n%s", transcript, mentions, text)
		}
	}
}
