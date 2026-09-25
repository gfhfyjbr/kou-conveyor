package anthropic

import (
	"encoding/base64"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"fmt"
	"maps"
	"slices"
	"strings"

	sdk "github.com/anthropics/anthropic-sdk-go"
	"github.com/anthropics/anthropic-sdk-go/packages/param"
	"github.com/anthropics/anthropic-sdk-go/shared/constant"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
)

// model is what the request shape depends on for one model ID.
type model struct {
	// claude models take thinking controls and cache breakpoints; other models
	// behind Messages-compatible endpoints get a plain request.
	claude bool
	// adaptive models think on their own and take an effort level; older
	// Claude models take a thinking budget instead.
	adaptive bool
	// noXHigh models stop at high before max.
	noXHigh bool
	// thinks is false for Claude models without extended thinking.
	thinks    bool
	maxOutput int64
	// contextWindow is how many tokens a request and its response can hold.
	contextWindow int64
	// fallbacks marks models whose refusals Anthropic can re-serve on another model.
	fallbacks bool
}

func describeModel(id string) model {
	id = strings.ToLower(id)
	start := strings.Index(id, "claude-")
	if start < 0 {
		return model{}
	}
	// Gateways prefix vendors ("anthropic/claude-…") and spell versions with dots.
	name := strings.ReplaceAll(id[start:], ".", "-")
	prefixed := func(prefixes ...string) bool {
		return slices.ContainsFunc(prefixes, func(prefix string) bool { return strings.HasPrefix(name, prefix) })
	}
	switch {
	case prefixed("claude-opus-5", "claude-fable-5", "claude-mythos-5", "claude-sonnet-5", "claude-mythos-preview",
		"claude-opus-4-8", "claude-opus-4-7"):
		return model{claude: true, adaptive: true, thinks: true, maxOutput: 128000, contextWindow: 1_000_000,
			fallbacks: prefixed("claude-opus-5", "claude-fable-5")}
	case prefixed("claude-opus-4-6", "claude-sonnet-4-6"):
		return model{claude: true, adaptive: true, noXHigh: true, thinks: true, maxOutput: 128000, contextWindow: 200_000}
	case prefixed("claude-3-haiku"):
		return model{claude: true, maxOutput: 4096, contextWindow: 200_000}
	case prefixed("claude-3-5"):
		return model{claude: true, maxOutput: 8192, contextWindow: 200_000}
	case prefixed("claude-opus-4-0", "claude-opus-4-1", "claude-opus-4-2"):
		return model{claude: true, thinks: true, maxOutput: 32000, contextWindow: 200_000}
	default:
		return model{claude: true, thinks: true, maxOutput: 64000, contextWindow: 200_000}
	}
}

// ContextWindow returns how many tokens a request to a Claude model and its
// response can hold, or 0 for a model ID that does not name a Claude model.
// Gateway spellings such as "anthropic/claude-opus-4.1" are understood.
func ContextWindow(id string) int64 {
	return describeModel(id).contextWindow
}

// Thinking budgets for Claude models that predate effort levels.
var thinkingBudgets = map[llm.ReasoningEffort]int64{
	llm.ReasoningEffortLow:    4096,
	llm.ReasoningEffortMedium: 10000,
	llm.ReasoningEffortHigh:   20000,
	llm.ReasoningEffortXHigh:  32000,
	llm.ReasoningEffortMax:    48000,
}

// params builds the API request. room, when positive, caps the response to
// what the context window leaves.
func (client *Client) params(request llm.Request, keepThinking bool, room int64) (sdk.BetaMessageNewParams, error) {
	id := strings.TrimSpace(request.Model.ID)
	if id == "" {
		id = DefaultModel
	}
	effort := request.Model.ReasoningEffort
	if effort != "" && !effort.Valid() {
		return sdk.BetaMessageNewParams{}, fmt.Errorf("unsupported reasoning effort %q", effort)
	}
	described := describeModel(id)
	system, messages, err := conversation(ownThinking(request.Input, id), keepThinking)
	if err != nil {
		return sdk.BetaMessageNewParams{}, err
	}
	if len(messages) == 0 {
		return sdk.BetaMessageNewParams{}, errors.New("request has no messages for the model")
	}
	tools, err := requestTools(request.Tools, client.official && described.claude)
	if err != nil {
		return sdk.BetaMessageNewParams{}, err
	}
	params := sdk.BetaMessageNewParams{
		Model:     sdk.Model(id),
		MaxTokens: client.maxOutputTokens(request.Model, described),
		System:    system,
		Messages:  messages,
		Tools:     tools,
	}
	if room > 0 {
		params.MaxTokens = min(params.MaxTokens, room)
	}
	switch {
	case described.adaptive:
		// Summaries keep the reasoning visible; the full thinking is never returned.
		params.Thinking.OfAdaptive = &sdk.BetaThinkingConfigAdaptiveParam{Display: sdk.BetaThinkingConfigAdaptiveDisplaySummarized}
		if effort == llm.ReasoningEffortXHigh && described.noXHigh {
			effort = llm.ReasoningEffortHigh
		}
		if effort != "" {
			params.OutputConfig.Effort = sdk.BetaOutputConfigEffort(effort)
		}
	case described.thinks && effort != "":
		// The budget must leave room for the answer itself.
		if budget := min(thinkingBudgets[effort], params.MaxTokens*3/4); budget >= 1024 {
			params.Thinking.OfEnabled = &sdk.BetaThinkingConfigEnabledParam{BudgetTokens: budget}
		}
	}
	if described.claude {
		// The system prompt (and the tools before it) is the stable prefix; the
		// last block moves with the conversation so each turn caches for the next.
		if len(params.System) != 0 {
			params.System[len(params.System)-1].CacheControl = sdk.NewBetaCacheControlEphemeralParam()
		}
		// The previous request ended at the user turn before the latest answer;
		// marking it too keeps that cache in reach however long this turn got.
		marked := 0
		for index := len(params.Messages) - 1; index >= 0 && marked < 2; index-- {
			message := params.Messages[index]
			if index != len(params.Messages)-1 && message.Role != sdk.BetaMessageParamRoleUser {
				continue
			}
			if control := message.Content[len(message.Content)-1].GetCacheControl(); control != nil {
				*control = sdk.NewBetaCacheControlEphemeralParam()
			}
			marked++
		}
	}
	if described.fallbacks && client.fallbacks && !client.noFallbacks.Load() {
		params.Fallbacks.OfDefault = constant.ValueOf[constant.Default]()
		params.Betas = []sdk.AnthropicBeta{sdk.AnthropicBetaServerSideFallback2026_07_01}
	}
	return params, nil
}

func (client *Client) maxOutputTokens(settings llm.Model, described model) int64 {
	switch {
	case settings.MaxOutputTokens != nil && *settings.MaxOutputTokens > 0:
		return *settings.MaxOutputTokens
	case client.maxTokens > 0:
		return client.maxTokens
	case !described.claude:
		return 16384
	}
	// Agentic turns think and act within one limit; the deepest efforts need more room.
	limit := int64(64000)
	if settings.ReasoningEffort == llm.ReasoningEffortXHigh || settings.ReasoningEffort == llm.ReasoningEffortMax {
		limit = 128000
	}
	return min(limit, described.maxOutput)
}

func requestTools(source []llm.Tool, eager bool) ([]sdk.BetaToolUnionParam, error) {
	tools := make([]sdk.BetaToolUnionParam, 0, len(source))
	for _, tool := range source {
		if tool.Type != llm.ToolFunction {
			return nil, fmt.Errorf("tool %q: the Messages API client supports function tools only, not %q", tool.Name, tool.Type)
		}
		schema, err := inputSchema(tool.Parameters)
		if err != nil {
			return nil, fmt.Errorf("tool %q: %w", tool.Name, err)
		}
		converted := &sdk.BetaToolParam{Name: tool.Name, InputSchema: schema}
		if tool.Description != "" {
			converted.Description = sdk.String(tool.Description)
		}
		if eager {
			// Large inputs, such as whole files, stream as they are written
			// instead of arriving in one burst. The harness validates every call
			// against its schema, and truncated calls are dropped below.
			converted.EagerInputStreaming = sdk.Bool(true)
		}
		tools = append(tools, sdk.BetaToolUnionParam{OfTool: converted})
	}
	return tools, nil
}

// inputSchema encodes a tool's JSON schema with sorted keys. Tools head the
// cached prefix, and the SDK's own encoding of extra schema keywords follows
// map order, which would change the prefix from one request to the next.
func inputSchema(parameters map[string]any) (sdk.BetaToolInputSchemaParam, error) {
	schema := maps.Clone(parameters)
	if schema == nil {
		schema = map[string]any{}
	}
	schema["type"] = "object"
	encoded, err := json.Marshal(schema, json.Deterministic(true))
	if err != nil {
		return sdk.BetaToolInputSchemaParam{}, fmt.Errorf("encode input schema: %w", err)
	}
	return param.Override[sdk.BetaToolInputSchemaParam](rawJSON(encoded)), nil
}

// conversation maps harness items to a system prompt and alternating turns.
//
// The harness records tool results as they arrive, which the API cannot
// express directly: a call must be answered in the very next user turn,
// exactly once, ahead of any other content. Results for the calls of the
// preceding turn are therefore gathered first. A result that arrives after its
// call was answered (a long-running tool first reports a placeholder) becomes
// text, and a call left without any result is answered with an error. Every
// step depends only on the items, so earlier turns render identically on every
// request, which the prompt cache and thinking signatures require.
func conversation(items []llm.Item, keepThinking bool) ([]sdk.BetaTextBlockParam, []sdk.BetaMessageParam, error) {
	// Thinking recorded before the last reset was left out of the history
	// that all later thinking is bound to, so it stays out.
	reset := -1
	for index, item := range items {
		if isThinkingReset(item) {
			reset = index
		}
	}
	var turns turnBuilder
	for index, item := range items {
		if err := turns.add(item, keepThinking && index > reset); err != nil {
			return nil, nil, fmt.Errorf("input item %d: %w", index, err)
		}
	}
	turns.close()
	return turns.system, turns.messages, nil
}

// ownThinking leaves out the thinking another model wrote. A session may
// change models from one prompt to the next, and a thinking block's
// signature holds for the model that wrote it: another rejects the block,
// which would cost a request and every block before it. Thinking that does
// not say which model wrote it stays, for the API to judge.
func ownThinking(items []llm.Item, model string) []llm.Item {
	foreign := func(item llm.Item) bool {
		return item.Type == llm.ItemReasoning && item.Model != "" && item.Model != model
	}
	if !slices.ContainsFunc(items, foreign) {
		return items
	}
	return slices.DeleteFunc(slices.Clone(items), foreign)
}

const thinkingResetType = "thinking_reset"

// thinkingReset marks where replayed thinking was rejected and left out. It
// is a reasoning item without a summary, so front-ends show nothing and
// other APIs skip it like any reasoning they did not record.
func thinkingReset() llm.Item {
	return llm.Item{Type: llm.ItemReasoning, Data: llm.Reasoning{Raw: jsontext.Value(`{"type":"` + thinkingResetType + `"}`)}}
}

func isThinkingReset(item llm.Item) bool {
	reasoning, ok := item.Data.(llm.Reasoning)
	if item.Type != llm.ItemReasoning || !ok || len(reasoning.Raw) == 0 {
		return false
	}
	var raw struct {
		Type string `json:"type"`
	}
	return json.Unmarshal(reasoning.Raw, &raw) == nil && raw.Type == thinkingResetType
}

type turnBuilder struct {
	system   []sdk.BetaTextBlockParam
	messages []sdk.BetaMessageParam
	started  bool

	role      sdk.BetaMessageParamRole
	assistant []sdk.BetaContentBlockParamUnion
	// calls lists the tool calls of the last assistant turn, which the open
	// user turn answers.
	calls   []string
	results map[string]sdk.BetaContentBlockParamUnion
	user    []sdk.BetaContentBlockParamUnion
}

func (turns *turnBuilder) add(item llm.Item, keepThinking bool) error {
	switch item.Type {
	case llm.ItemMessage:
		message, ok := item.Data.(llm.Message)
		if !ok {
			return fmt.Errorf("message item data must be llm.Message, got %T", item.Data)
		}
		block, ok := textBlock(message.Text)
		switch message.Role {
		case llm.RoleSystem:
			if !turns.started {
				if ok {
					turns.system = append(turns.system, *block.OfText)
				}
				return nil
			}
			// Instructions that arrive mid-conversation reach the model as user text.
			turns.addUser(block, ok)
		case llm.RoleUser:
			turns.addImages(message.Images)
			turns.addUser(block, ok)
		case llm.RoleAssistant:
			turns.addAssistant(block, ok)
		default:
			return fmt.Errorf("unsupported message role %q", message.Role)
		}
	case llm.ItemReasoning:
		reasoning, ok := item.Data.(llm.Reasoning)
		if !ok {
			return fmt.Errorf("reasoning item data must be llm.Reasoning, got %T", item.Data)
		}
		if keepThinking {
			turns.addAssistant(thinkingBlock(reasoning.Raw))
		}
	case llm.ItemToolCall:
		call, ok := item.Data.(llm.ToolCall)
		if !ok {
			return fmt.Errorf("tool_call item data must be llm.ToolCall, got %T", item.Data)
		}
		turns.addAssistant(sdk.BetaContentBlockParamUnion{OfToolUse: &sdk.BetaToolUseBlockParam{
			ID: call.CallID, Name: call.Name, Input: toolInput(call.Arguments),
		}}, true)
		turns.calls = append(turns.calls, call.CallID)
	case llm.ItemToolResult:
		result, ok := item.Data.(llm.ToolResult)
		if !ok {
			return fmt.Errorf("tool_result item data must be llm.ToolResult, got %T", item.Data)
		}
		turns.addResult(result)
	default:
		return fmt.Errorf("unsupported input item type %q", item.Type)
	}
	return nil
}

func (turns *turnBuilder) addAssistant(block sdk.BetaContentBlockParamUnion, ok bool) {
	if !ok {
		return
	}
	turns.started = true
	if turns.role == sdk.BetaMessageParamRoleUser {
		turns.closeUser()
	}
	turns.role = sdk.BetaMessageParamRoleAssistant
	turns.assistant = append(turns.assistant, block)
}

func (turns *turnBuilder) openUser() {
	turns.started = true
	if turns.role == sdk.BetaMessageParamRoleAssistant {
		turns.closeAssistant()
	}
	turns.role = sdk.BetaMessageParamRoleUser
}

func (turns *turnBuilder) addUser(block sdk.BetaContentBlockParamUnion, ok bool) {
	if !ok {
		return
	}
	turns.openUser()
	turns.user = append(turns.user, block)
}

// addImages puts the images of a user's message ahead of its text, each
// after its label, which is how the text refers to it: Claude reads an image
// best before what is said about it.
func (turns *turnBuilder) addImages(images []llm.Image) {
	for _, image := range images {
		turns.addUser(textBlock(image.Label))
		if block, ok := imageBlock(image.URL); ok {
			turns.addUser(sdk.BetaContentBlockParamUnion{OfImage: block}, true)
		} else {
			turns.addUser(textBlock("[image omitted: the Messages API accepts PNG, JPEG, GIF and WebP images]"))
		}
	}
}

func (turns *turnBuilder) addResult(result llm.ToolResult) {
	turns.openUser()
	content := toolResultContent(result.Output)
	if _, answered := turns.results[result.CallID]; !answered && slices.Contains(turns.calls, result.CallID) {
		if turns.results == nil {
			turns.results = map[string]sdk.BetaContentBlockParamUnion{}
		}
		turns.results[result.CallID] = sdk.BetaContentBlockParamUnion{OfToolResult: &sdk.BetaToolResultBlockParam{
			ToolUseID: result.CallID, Content: content,
		}}
		return
	}
	header, _ := textBlock(fmt.Sprintf("Result of tool call %s:", result.CallID))
	turns.user = append(turns.user, header)
	for _, part := range content {
		switch {
		case part.OfText != nil:
			turns.user = append(turns.user, sdk.BetaContentBlockParamUnion{OfText: part.OfText})
		case part.OfImage != nil:
			turns.user = append(turns.user, sdk.BetaContentBlockParamUnion{OfImage: part.OfImage})
		}
	}
}

func (turns *turnBuilder) closeAssistant() {
	if len(turns.assistant) != 0 {
		turns.messages = append(turns.messages, sdk.BetaMessageParam{Role: sdk.BetaMessageParamRoleAssistant, Content: turns.assistant})
	}
	turns.assistant = nil
	turns.role = ""
}

func (turns *turnBuilder) closeUser() {
	content := make([]sdk.BetaContentBlockParamUnion, 0, len(turns.calls)+len(turns.user))
	for _, call := range turns.calls {
		result, ok := turns.results[call]
		if !ok {
			result = sdk.BetaContentBlockParamUnion{OfToolResult: &sdk.BetaToolResultBlockParam{
				ToolUseID: call,
				Content:   toolResultContent([]llm.ToolResultOutput{{Kind: llm.ToolResultText, Value: "No result was recorded for this tool call."}}),
				IsError:   sdk.Bool(true),
			}}
		}
		content = append(content, result)
	}
	content = append(content, turns.user...)
	if len(content) != 0 {
		turns.messages = append(turns.messages, sdk.BetaMessageParam{Role: sdk.BetaMessageParamRoleUser, Content: content})
	}
	turns.calls, turns.results, turns.user = nil, nil, nil
	turns.role = ""
}

func (turns *turnBuilder) close() {
	switch turns.role {
	case sdk.BetaMessageParamRoleAssistant:
		turns.closeAssistant()
		if len(turns.calls) != 0 {
			turns.closeUser()
		}
	case sdk.BetaMessageParamRoleUser:
		turns.closeUser()
	}
}

// textBlock reports false for blank text, which the API rejects.
func textBlock(text string) (sdk.BetaContentBlockParamUnion, bool) {
	if strings.TrimSpace(text) == "" {
		return sdk.BetaContentBlockParamUnion{}, false
	}
	return sdk.BetaContentBlockParamUnion{OfText: &sdk.BetaTextBlockParam{Text: text}}, true
}

// thinkingBlock replays a thinking block exactly as the API returned it; its
// signature covers the text. Reasoning recorded by other providers is left out.
func thinkingBlock(raw jsontext.Value) (sdk.BetaContentBlockParamUnion, bool) {
	var block struct {
		Type      string `json:"type"`
		Thinking  string `json:"thinking"`
		Signature string `json:"signature"`
		Data      string `json:"data"`
	}
	if len(raw) == 0 || json.Unmarshal(raw, &block) != nil {
		return sdk.BetaContentBlockParamUnion{}, false
	}
	switch {
	case block.Type == "thinking" && block.Signature != "":
		return sdk.BetaContentBlockParamUnion{OfThinking: &sdk.BetaThinkingBlockParam{Thinking: block.Thinking, Signature: block.Signature}}, true
	case block.Type == "redacted_thinking" && block.Data != "":
		return sdk.BetaContentBlockParamUnion{OfRedactedThinking: &sdk.BetaRedactedThinkingBlockParam{Data: block.Data}}, true
	}
	return sdk.BetaContentBlockParamUnion{}, false
}

// rawJSON passes the model's own argument bytes through, so replayed calls
// keep the key order they were written in.
type rawJSON []byte

func (value rawJSON) MarshalJSON() ([]byte, error) { return value, nil }

func toolInput(arguments string) rawJSON {
	value := jsontext.Value(arguments)
	if value.Kind() == jsontext.KindBeginObject && value.IsValid() {
		return rawJSON(arguments)
	}
	// Tool inputs must be objects; keep a malformed call visible next to the
	// error the harness reported for it.
	encoded, _ := json.Marshal(struct {
		InvalidArguments string `json:"invalid_arguments"`
	}{arguments})
	return rawJSON(encoded)
}

var imageTypes = []sdk.BetaBase64ImageSourceMediaType{
	sdk.BetaBase64ImageSourceMediaTypeImagePNG, sdk.BetaBase64ImageSourceMediaTypeImageJPEG,
	sdk.BetaBase64ImageSourceMediaTypeImageGIF, sdk.BetaBase64ImageSourceMediaTypeImageWebP,
}

func toolResultContent(output []llm.ToolResultOutput) []sdk.BetaToolResultBlockParamContentUnion {
	content := make([]sdk.BetaToolResultBlockParamContentUnion, 0, len(output))
	for _, part := range output {
		switch part.Kind {
		case llm.ToolResultText:
			if strings.TrimSpace(part.Value) != "" {
				content = append(content, sdk.BetaToolResultBlockParamContentUnion{OfText: &sdk.BetaTextBlockParam{Text: part.Value}})
			}
		case llm.ToolResultImage:
			if image, ok := imageBlock(part.Value); ok {
				content = append(content, sdk.BetaToolResultBlockParamContentUnion{OfImage: image})
			} else {
				content = append(content, sdk.BetaToolResultBlockParamContentUnion{OfText: &sdk.BetaTextBlockParam{
					Text: "[image omitted: the Messages API accepts PNG, JPEG, GIF and WebP images]",
				}})
			}
		}
	}
	return content
}

// imageBlock accepts the harness's image references: base64 data URLs and
// http(s) URLs.
func imageBlock(reference string) (*sdk.BetaImageBlockParam, bool) {
	if rest, ok := strings.CutPrefix(reference, "data:"); ok {
		header, data, ok := strings.Cut(rest, ",")
		mediaType, encoding, _ := strings.Cut(header, ";")
		mediaType = strings.ToLower(mediaType)
		if mediaType == "image/jpg" {
			mediaType = "image/jpeg"
		}
		if !ok || encoding != "base64" || !slices.Contains(imageTypes, sdk.BetaBase64ImageSourceMediaType(mediaType)) {
			return nil, false
		}
		if _, err := base64.StdEncoding.DecodeString(data); err != nil {
			return nil, false
		}
		return &sdk.BetaImageBlockParam{Source: sdk.BetaImageBlockParamSourceUnion{OfBase64: &sdk.BetaBase64ImageSourceParam{
			Data: data, MediaType: sdk.BetaBase64ImageSourceMediaType(mediaType),
		}}}, true
	}
	if strings.HasPrefix(reference, "https://") || strings.HasPrefix(reference, "http://") {
		return &sdk.BetaImageBlockParam{Source: sdk.BetaImageBlockParamSourceUnion{OfURL: &sdk.BetaURLImageSourceParam{URL: reference}}}, true
	}
	return nil, false
}
