package agentrunner

import (
	"bytes"
	"context"
	"encoding/base64"
	"encoding/json/jsontext"
	"encoding/json/v2"
	"errors"
	"flag"
	"fmt"
	"io"
	"io/fs"
	"os"
	"path/filepath"
	"slices"
	"strconv"
	"strings"
	"sync"
	"time"
	"uuid"

	"github.com/gfhfyjbr/kou-conveyor/harness/contextbuilder"
	"github.com/gfhfyjbr/kou-conveyor/harness/coordinator"
	"github.com/gfhfyjbr/kou-conveyor/harness/inbox"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm"
	"github.com/gfhfyjbr/kou-conveyor/harness/llm/responsesapi"
	"github.com/gfhfyjbr/kou-conveyor/harness/operation"
	"github.com/gfhfyjbr/kou-conveyor/harness/plugin"
	"github.com/gfhfyjbr/kou-conveyor/harness/session"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore"
	"github.com/gfhfyjbr/kou-conveyor/harness/sessionstore/localfile"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/bash"
	"github.com/gfhfyjbr/kou-conveyor/harness/tool/viewimage"
)

const (
	defaultProvider           = "openai"
	defaultSessionDirectory   = ".harness/sessions"
	llmAPIKeyEnvironment      = "KOU_CONVEYOR_LLM_API_KEY"
	llmBaseURLEnvironment     = "KOU_CONVEYOR_LLM_BASE_URL"
	llmModelEnvironment       = "KOU_CONVEYOR_LLM_MODEL"
	llmProviderEnvironment    = "KOU_CONVEYOR_LLM_PROVIDER"
	llmMaxAttemptsEnvironment = "KOU_CONVEYOR_LLM_MAX_ATTEMPTS"
	llmMaxTokensEnvironment   = "KOU_CONVEYOR_LLM_MAX_TOKENS"
	contextWindowEnvironment  = "KOU_CONVEYOR_CONTEXT_WINDOW"
	autoCompactEnvironment    = "KOU_CONVEYOR_AUTO_COMPACT"
)

// defaultSteerWait is how long a message sent while the agent works waits
// for the tool calls it is making: long enough for tests and builds, short
// enough that a server that never exits does not hold it.
const defaultSteerWait = 2 * time.Minute

const (
	// defaultContextWindow is assumed for models no provider knows.
	defaultContextWindow = 128_000
	// autoCompactReserve is how much of the context window automatic
	// compaction keeps free unless configured, as Claude Code does: room for
	// the summary, up to 20,000 tokens, and 13,000 for what arrives before
	// the next turn. A small window keeps a quarter of itself free instead.
	autoCompactReserve = 33_000
)

const defaultSystemPrompt = `You are an AI agent running inside an isolated sandbox container.

## Guidelines
- Save output files to the workspace root.
- For large datasets, inspect a sample first before processing everything.
`

type Client interface {
	llm.Adapter
	Close() error
}

type Provider struct {
	Name              string
	BaseURL           string
	DefaultModel      string
	APIKeyEnvironment string // Empty delegates authentication to NewClient.
	NewClient         func(apiKey, baseURL string, maxAttempts int, getenv func(string) string) (Client, error)
	// ContextWindow returns how many tokens a request to a model can hold, or
	// 0 when the provider does not know the model. Nil knows no model.
	ContextWindow func(model string) int64
}

type Request struct {
	Messages               []RequestMessage `json:"messages"`
	Prompt                 *string          `json:"prompt"`
	SystemPrompt           *string          `json:"system_prompt"`
	Model                  string           `json:"model"`
	MaxAttempts            *int             `json:"max_attempts"`
	SessionID              *string          `json:"session_id"`
	ThinkingLevel          string           `json:"thinking_level"`
	IncludePartialMessages *bool            `json:"include_partial_messages"`
	ExtraAllowedTools      []string         `json:"extra_allowed_tools"`
	DisallowedTools        []string         `json:"disallowed_tools"`
	// Compact summarizes the session's conversation before any messages run,
	// freeing the context it takes; CompactInstructions tell the summary what
	// to focus on.
	Compact             bool   `json:"compact"`
	CompactInstructions string `json:"compact_instructions"`
}

type RequestMessage struct {
	Role      string  `json:"role"`
	Content   string  `json:"content"`
	MessageID *string `json:"message_id"`
	// Images go with the message: the model sees each beside the text,
	// after its label.
	Images []RequestImage `json:"images"`
}

// RequestImage is a picture for the model: the image's bytes (base64 in
// JSON) and their media type, or the http(s) URL of an image. Label is how
// the message's text refers to it; it defaults to "[Image n]", n counting
// the message's images from 1.
type RequestImage struct {
	Label     string `json:"label"`
	MediaType string `json:"media_type"`
	Data      []byte `json:"data"`
	URL       string `json:"url"`
}

const (
	// maxImages bounds the images of one message, and maxImageBytes the size
	// of one; providers take images of a few megabytes.
	maxImages     = 20
	maxImageBytes = 20 << 20
)

// imageMediaTypes are the image formats every provider takes.
var imageMediaTypes = []string{"image/png", "image/jpeg", "image/gif", "image/webp"}

// messageImages checks the images of a message and returns the references
// the model gets: data URLs of their bytes, or their URLs.
func messageImages(images []RequestImage) ([]llm.Image, error) {
	if len(images) > maxImages {
		return nil, fmt.Errorf("a message takes at most %d images, not %d", maxImages, len(images))
	}
	references := make([]llm.Image, 0, len(images))
	for index, image := range images {
		label := strings.TrimSpace(image.Label)
		if label == "" {
			label = fmt.Sprintf("[Image %d]", index+1)
		}
		switch url := strings.TrimSpace(image.URL); {
		case url != "" && len(image.Data) != 0:
			return nil, fmt.Errorf("image %d has both data and a url", index+1)
		case url != "":
			if !strings.HasPrefix(url, "https://") && !strings.HasPrefix(url, "http://") {
				return nil, fmt.Errorf("image %d: url must be an http(s) URL", index+1)
			}
			references = append(references, llm.Image{Label: label, URL: url})
		case len(image.Data) == 0:
			return nil, fmt.Errorf("image %d has neither data nor a url", index+1)
		case len(image.Data) > maxImageBytes:
			return nil, fmt.Errorf("image %d is %d bytes; an image takes at most %d", index+1, len(image.Data), maxImageBytes)
		default:
			mediaType := strings.ToLower(strings.TrimSpace(image.MediaType))
			if mediaType == "image/jpg" {
				mediaType = "image/jpeg"
			}
			if !slices.Contains(imageMediaTypes, mediaType) {
				return nil, fmt.Errorf("image %d: media_type must be one of %s", index+1, strings.Join(imageMediaTypes, ", "))
			}
			references = append(references, llm.Image{
				Label: label, URL: "data:" + mediaType + ";base64," + base64.StdEncoding.EncodeToString(image.Data),
			})
		}
	}
	return references, nil
}

// messagePayload is the payload of the input a message becomes.
func messagePayload(message RequestMessage) (jsontext.Value, error) {
	images, err := messageImages(message.Images)
	if err != nil {
		return nil, err
	}
	return contextbuilder.ExternalPayload(message.Content, images)
}

type environmentChange struct {
	name    string
	value   string
	present bool
}

type environmentScope struct {
	changes []environmentChange
}

type errorEvent struct {
	Type    string `json:"type"`
	Message string `json:"message"`
}

type sessionObserver struct {
	sessionID session.ID
	output    io.Writer
	cancel    context.CancelFunc

	mu  sync.Mutex
	err error
}

func RunMain(
	ctx context.Context,
	args []string,
	getenv func(string) string,
	environ func() []string,
	input io.Reader,
	output io.Writer,
	stderr io.Writer,
	config Config,
) int {
	err := Run(ctx, args, getenv, environ, input, output, stderr, config)
	if err == nil {
		return 0
	}
	if errors.Is(err, flag.ErrHelp) {
		return 0
	}
	if context.Cause(ctx) != nil {
		return 130
	}
	encoded, encodeErr := json.Marshal(errorEvent{Type: "error", Message: err.Error()})
	if encodeErr != nil {
		err = errors.Join(err, fmt.Errorf("encode error event: %w", encodeErr))
	} else if _, writeErr := fmt.Fprintf(output, "%s\n", encoded); writeErr != nil {
		err = errors.Join(err, fmt.Errorf("write error event: %w", writeErr))
	}
	prefix := ""
	if config.Name != "" {
		prefix = config.Name + ": "
	}
	if _, writeErr := fmt.Fprintf(stderr, "%s%v\n", prefix, err); writeErr != nil {
		return 1
	}
	return 1
}

func Run(
	ctx context.Context,
	args []string,
	getenv func(string) string,
	environ func() []string,
	input io.Reader,
	output io.Writer,
	flagOutput io.Writer,
	config Config,
) (runErr error) {
	if strings.TrimSpace(config.Name) == "" {
		return errors.New("runner name must be set")
	}
	if config.ParseRequest == nil {
		return errors.New("request parser must be set")
	}
	flags := flag.NewFlagSet(config.Name, flag.ContinueOnError)
	flags.SetOutput(flagOutput)
	var usageErr error
	flags.Usage = func() {
		usageErr = writeUsage(flags)
	}
	var prompt *string
	flags.Func("p", "send a request with the given `prompt` without reading stdin", func(value string) error {
		prompt = &value
		return nil
	})
	sessionDirectory := flags.String("session-directory", defaultSessionDirectory, "directory containing session files")
	workspaceDirectory := flags.String("workspace", ".", "agent workspace and Bash working directory")
	logDirectory := flags.String("log-directory", "", "session JSONL log directory; defaults to <workspace>/logs")
	toolHeartbeatInterval := flags.Duration("tool-heartbeat-interval", 10*time.Minute, "tool-wait heartbeat interval (0 disables)")
	listProviders := flags.Bool("providers", false, "print the providers this runner supports, one per line, and exit")
	listPluginsFlag := flags.Bool("list-plugins", false, "print the plugins a run in the workspace would find, one JSON object per line, and exit")
	steer := flags.Bool("steer", false, "keep reading stdin after the request: each further JSON object, {\"content\": string, \"message_id\"?: UUID}, is a message for the running agent, which reads it after the tool calls it is making")
	steerWait := flags.Duration("steer-wait", defaultSteerWait, "the longest a message sent with -steer waits for the tool calls the agent is making before it goes in with the results that are in (0 waits for them)")
	if err := flags.Parse(args); err != nil {
		if usageErr != nil {
			if errors.Is(err, flag.ErrHelp) {
				return usageErr
			}
			return errors.Join(err, usageErr)
		}
		return err
	}
	if flags.NArg() > 1 {
		return errors.New("expected at most one positional JSON request")
	}
	if prompt != nil && flags.NArg() != 0 {
		return errors.New("-p cannot be combined with a positional JSON request")
	}
	if *toolHeartbeatInterval < 0 {
		return errors.New("tool heartbeat interval must not be negative")
	}
	if *steerWait < 0 {
		return errors.New("steer wait must not be negative")
	}
	if *listProviders {
		// Front-ends ask before a run, so a runner older than they are is
		// reported as such instead of failing the run.
		for _, provider := range config.Providers {
			if _, err := fmt.Fprintln(output, provider.Name); err != nil {
				return err
			}
		}
		return nil
	}
	if *listPluginsFlag {
		workspace, err := filepath.Abs(strings.TrimSpace(*workspaceDirectory))
		if err != nil {
			return fmt.Errorf("resolve workspace: %w", err)
		}
		return listPlugins(output, discoverPlugins(config, getenv, workspace))
	}

	// With -steer, messages for the running agent follow on stdin: after the
	// request when it comes from stdin too.
	var steering *jsontext.Decoder
	if *steer {
		steering = jsontext.NewDecoder(input)
	}
	if prompt != nil {
		encoded, err := json.Marshal(struct {
			Prompt string `json:"prompt"`
		}{Prompt: *prompt})
		if err != nil {
			return fmt.Errorf("encode prompt request: %w", err)
		}
		input = bytes.NewReader(encoded)
	} else if flags.NArg() == 1 {
		input = strings.NewReader(flags.Arg(0))
	} else if steering != nil {
		request, err := steering.ReadValue()
		switch {
		case errors.Is(err, io.EOF):
			return errors.New("empty input")
		case err != nil:
			return fmt.Errorf("invalid JSON: %w", err)
		}
		input = bytes.NewReader(bytes.Clone(request))
	}
	parsed, newTools, err := config.ParseRequest(input)
	if err != nil {
		return err
	}
	if newTools == nil {
		return errors.New("request parser returned no tool factory")
	}
	messages, err := validateRequest(parsed)
	if err != nil {
		return err
	}
	workspace, err := filepath.Abs(strings.TrimSpace(*workspaceDirectory))
	if err != nil {
		return fmt.Errorf("resolve workspace: %w", err)
	}
	workspaceInfo, err := os.Stat(workspace)
	if err != nil {
		return fmt.Errorf("inspect workspace: %w", err)
	}
	if !workspaceInfo.IsDir() {
		return fmt.Errorf("workspace %q is not a directory", workspace)
	}
	environment, err := loadDotEnv(filepath.Join(workspace, ".env"))
	if err != nil {
		return err
	}
	defer func() {
		if err := environment.Close(); err != nil {
			runErr = errors.Join(runErr, err)
		}
	}()
	maxAttempts, err := resolveMaxAttempts(parsed.MaxAttempts, getenv)
	if err != nil {
		return err
	}
	providerName := strings.TrimSpace(getenv(llmProviderEnvironment))
	if providerName == "" {
		providerName = defaultProvider
	}
	selected, err := selectProvider(config.Providers, providerName)
	if err != nil {
		return err
	}
	configuredBaseURL := strings.TrimSpace(getenv(llmBaseURLEnvironment))
	if configuredBaseURL == "" {
		configuredBaseURL = selected.BaseURL
	}

	model := strings.TrimSpace(parsed.Model)
	if model == "" {
		model = strings.TrimSpace(getenv(llmModelEnvironment))
	}
	if model == "" {
		model = selected.DefaultModel
	}
	if model == "" {
		return fmt.Errorf("model must be set in the request or %s", llmModelEnvironment)
	}
	contextWindow, autoCompact, err := compactionLimits(getenv, selected, model)
	if err != nil {
		return err
	}
	var apiKey string
	if selected.APIKeyEnvironment != "" {
		apiKey = getenv(llmAPIKeyEnvironment)
		if strings.TrimSpace(apiKey) == "" {
			apiKey = getenv(selected.APIKeyEnvironment)
		}
		if strings.TrimSpace(apiKey) == "" {
			return fmt.Errorf(
				"%s or %s must be set",
				llmAPIKeyEnvironment,
				selected.APIKeyEnvironment,
			)
		}
	}
	client, err := selected.NewClient(apiKey, configuredBaseURL, maxAttempts, getenv)
	if err != nil {
		return fmt.Errorf("create %s client: %w", selected.Name, err)
	}
	// Tools inherit the runner's environment. The harness's own key variable
	// is for the runner alone (the cockpits hand saved keys over in it), so it
	// goes before any tool can print it.
	if err := environment.unset(llmAPIKeyEnvironment); err != nil {
		return err
	}
	defer func() {
		if err := client.Close(); err != nil {
			runErr = errors.Join(runErr, fmt.Errorf("close %s client: %w", selected.Name, err))
		}
	}()

	storeDirectory, err := filepath.Abs(strings.TrimSpace(*sessionDirectory))
	if err != nil {
		return fmt.Errorf("resolve session directory: %w", err)
	}
	store, err := localfile.New(storeDirectory)
	if err != nil {
		return fmt.Errorf("open session store: %w", err)
	}
	sessionID, restored, err := openSession(ctx, store, parsed.SessionID, parsed.Compact)
	if err != nil {
		return err
	}
	logFile, err := openDatetimeLog(resolveLogDirectory(workspace, *logDirectory), time.Now())
	if err != nil {
		return err
	}
	defer func() {
		if err := logFile.Close(); err != nil {
			runErr = errors.Join(runErr, fmt.Errorf("close session log: %w", err))
		}
	}()
	observedOutput := io.MultiWriter(logFile, output)

	runContext, cancel := context.WithCancel(ctx)
	defer cancel()
	operationDirectory := filepath.Join(storeDirectory, "operations", string(sessionID))
	if err := os.MkdirAll(operationDirectory, 0o700); err != nil {
		return fmt.Errorf("create operation directory: %w", err)
	}
	shell := strings.TrimSpace(getenv("SHELL"))
	if shell == "" {
		shell = "/bin/sh"
	}
	// Plugins are followed while the run goes on (liveplugins.go): the
	// fingerprint of where they come from is taken before they are read, so
	// a change while they are read is seen at the next turn.
	options, optionProblems := pluginOptions(config, getenv, workspace)
	pluginFingerprint := plugin.Fingerprint(plugin.Sources(options)...)
	plugins := plugin.Discover(options)
	plugins.Errors = append(optionProblems, plugins.Errors...)
	skills, skillErrors := tool.DiscoverSkills(filepath.Join(workspace, ".harness", "skills"))
	// The core plugin brings the built-in tools. SkillUse is there for
	// skills that come with a plugin later; the model is offered it only
	// while there are skills.
	var names []string
	if activePlugin(plugins, plugin.CoreName) {
		names = []string{tool.BashName, tool.ViewImageName, tool.SkillUseName}
	}
	toolConfig := ToolConfig{
		SessionID: sessionID, Getenv: getenv, Names: names,
		Translators: tool.StaticTranslators{
			Bash: bash.New(bash.Config{
				Shell:         shell,
				Directory:     workspace,
				BaseDirectory: operationDirectory,
			}),
			ViewImage: viewimage.New(viewimage.Config{Directory: workspace}),
		},
	}
	configuredTools, err := newTools(runContext, toolConfig)
	if err != nil {
		return err
	}
	defer func() {
		cancel()
		if configuredTools.Close != nil {
			runErr = errors.Join(runErr, configuredTools.Close())
		}
	}()
	registry := configuredTools.Registry
	_, skillUse := registry.Resolve(tool.SkillUseName)
	if skillUse {
		for _, skill := range skills {
			if _, err := registry.RegisterSkill(skill); err != nil {
				return fmt.Errorf("register skill %q: %w", skill.Path, err)
			}
		}
	}
	for _, skillErr := range skillErrors {
		if _, err := fmt.Fprintf(flagOutput, "skill error> %s\n", skillErr); err != nil {
			return fmt.Errorf("write skill error: %w", err)
		}
	}
	var staticTools []llm.Tool
	for _, definition := range registry.StaticDefinitions() {
		staticTools = append(staticTools, definition.Tool)
	}

	operations := operation.NewLocalOperationManager(runContext, configuredTools.RemoteJobs...)
	inputs, err := inbox.New(runContext, restored.ExternalInputIDs)
	if err != nil {
		return fmt.Errorf("open inbox: %w", err)
	}
	settingsPayload, err := json.Marshal(inbox.ControlMessage{
		Mode: inbox.UpdateSettings,
		Parameters: inbox.Settings{
			ReasoningEffort: reasoningEffort(parsed.ThinkingLevel),
		},
	})
	if err != nil {
		return fmt.Errorf("encode settings: %w", err)
	}
	if err := inputs.Submit(runContext, inbox.Input{
		ID: inbox.ID(uuid.New().String()), Kind: inbox.InputControl, Payload: settingsPayload,
	}); err != nil {
		return fmt.Errorf("submit settings: %w", err)
	}
	if parsed.Compact {
		compactPayload, err := json.Marshal(inbox.ControlMessage{
			Mode: inbox.Compact, Reason: strings.TrimSpace(parsed.CompactInstructions),
		})
		if err != nil {
			return fmt.Errorf("encode compaction request: %w", err)
		}
		if err := inputs.Submit(runContext, inbox.Input{
			ID: inbox.ID(uuid.New().String()), Kind: inbox.InputControl, Payload: compactPayload,
		}); err != nil {
			return fmt.Errorf("submit compaction request: %w", err)
		}
	}
	for index, message := range messages {
		payload, err := messagePayload(message)
		if err != nil {
			return fmt.Errorf("encode message %d: %w", index, err)
		}
		var messageID inbox.ID
		if message.MessageID != nil {
			messageID = inbox.ID(strings.TrimSpace(*message.MessageID))
		} else {
			messageID = inbox.ID(uuid.New().String())
		}
		if err := inputs.Submit(runContext, inbox.Input{
			ID: messageID, Kind: inbox.InputExternal, Payload: payload,
		}); err != nil {
			return fmt.Errorf("submit message %d: %w", index, err)
		}
	}

	stopPayload, err := json.Marshal(inbox.ControlMessage{Mode: inbox.StopWhenIdle})
	if err != nil {
		return fmt.Errorf("encode stop request: %w", err)
	}
	if err := inputs.Submit(runContext, inbox.Input{
		ID: inbox.ID(uuid.New().String()), Kind: inbox.InputControl, Payload: stopPayload,
	}); err != nil {
		return fmt.Errorf("submit stop request: %w", err)
	}
	if steering != nil {
		// Reading stdin may outlast the run; what it reports then is dropped.
		report := &lockedWriter{w: flagOutput}
		defer report.Close()
		go steerRun(runContext, steering, inputs, report)
	}

	builder := contextbuilder.NewBuilder(registry.Skills()...)
	builder.SetModel(llm.Model{
		ID:              model,
		ReasoningEffort: reasoningEffort(parsed.ThinkingLevel),
	})
	systemPrompt := defaultSystemPrompt
	if parsed.SystemPrompt != nil {
		systemPrompt = *parsed.SystemPrompt
	}
	// A compaction points the model to the whole conversation.
	builder.SetTranscript(store.Path(sessionID))
	// The plugins give the run tools, instructions and skills, now and as
	// they change.
	live := &livePlugins{
		options: options, parsed: parsed, sessionID: sessionID, workspace: workspace, operations: operationDirectory,
		registry: registry, builder: builder, output: flagOutput,
		systemPrompt: systemPrompt, static: staticTools, skillUse: skillUse,
		fingerprint: pluginFingerprint, tools: map[string]*liveTool{}, skills: map[string]tool.RegistrationID{},
	}
	for _, pluginErr := range live.apply(plugins) {
		if _, err := fmt.Fprintf(flagOutput, "plugin error> %s\n", pluginErr); err != nil {
			return fmt.Errorf("write plugin error: %w", err)
		}
	}
	live.shown = live.describe(plugins)
	var beforeTurn func()
	if strings.TrimSpace(getenv(WatchPluginsEnvironment)) != "0" {
		beforeTurn = live.beforeTurn
	}

	observer := &sessionObserver{
		sessionID: sessionID,
		output:    observedOutput,
		cancel:    cancel,
	}
	observerID := store.AddObserver(observer.Observe)
	defer store.RemoveObserver(observerID)
	current := coordinator.New(coordinator.Dependencies{
		BeforeTurn:            beforeTurn,
		ToolHeartbeatInterval: *toolHeartbeatInterval,
		ToolWaitLimit:         *steerWait,
		AutoCompactTokens:     autoCompact,
		ContextWindow:         contextWindow,
		SessionID:             sessionID,
		Inbox:                 inputs,
		Restored:              restored,
		Sessions:              store,
		ContextBuilder:        builder,
		LLM:                   client,
		Tools:                 registry,
		Operations:            operations,
	})
	coordinatorErr := current.Run(runContext)
	// Tools run in their own process groups. Canceling the run makes the
	// operation manager terminate them (TERM, then KILL after a grace period)
	// and close Updates once every process is gone; waiting for that keeps an
	// interrupted runner from exiting while its tools still run.
	cancel()
	for range operations.Updates() {
	}
	if observerErr := observer.Err(); observerErr != nil {
		return observerErr
	}
	if coordinatorErr != nil {
		return fmt.Errorf("run coordinator: %w", coordinatorErr)
	}
	return nil
}

func resolveMaxAttempts(requested *int, getenv func(string) string) (int, error) {
	maxAttempts := responsesapi.DefaultMaxAttempts
	if requested != nil {
		maxAttempts = *requested
	} else if value := strings.TrimSpace(getenv(llmMaxAttemptsEnvironment)); value != "" {
		parsed, err := strconv.Atoi(value)
		if err != nil {
			return 0, fmt.Errorf("parse %s: %w", llmMaxAttemptsEnvironment, err)
		}
		maxAttempts = parsed
	}
	if maxAttempts <= 0 {
		return 0, errors.New("max attempts must be positive")
	}
	return maxAttempts, nil
}

func selectProvider(providers []Provider, name string) (Provider, error) {
	for _, provider := range providers {
		if provider.Name == name {
			if provider.NewClient == nil {
				return Provider{}, fmt.Errorf("provider %q has no client factory", name)
			}
			return provider, nil
		}
	}
	names := make([]string, 0, len(providers))
	for _, provider := range providers {
		names = append(names, provider.Name)
	}
	return Provider{}, fmt.Errorf("unsupported provider %q; available providers: %s", name, strings.Join(names, ", "))
}

func DecodeRequest(input io.Reader, destination any) error {
	raw, err := io.ReadAll(input)
	if err != nil {
		return fmt.Errorf("read input: %w", err)
	}
	raw = bytes.TrimSpace(raw)
	if len(raw) == 0 {
		return errors.New("empty input")
	}
	if err := json.Unmarshal(raw, destination, json.RejectUnknownMembers(true)); err != nil {
		return fmt.Errorf("invalid JSON: %w", err)
	}
	return nil
}

func resolveLogDirectory(workspace, configured string) string {
	configured = strings.TrimSpace(configured)
	if configured == "" {
		return filepath.Join(workspace, "logs")
	}
	return configured
}

func openDatetimeLog(directory string, now time.Time) (*os.File, error) {
	if err := os.MkdirAll(directory, 0o700); err != nil {
		return nil, fmt.Errorf("create log directory: %w", err)
	}
	path := filepath.Join(directory, now.UTC().Format("20060102-150405")+".jsonl")
	file, err := os.OpenFile(path, os.O_CREATE|os.O_WRONLY|os.O_APPEND, 0o600)
	if err != nil {
		return nil, fmt.Errorf("open session log: %w", err)
	}
	return file, nil
}

func loadDotEnv(path string) (*environmentScope, error) {
	encoded, err := os.ReadFile(path)
	if err != nil {
		if errors.Is(err, fs.ErrNotExist) {
			return &environmentScope{}, nil
		}
		return nil, fmt.Errorf("read environment file: %w", err)
	}
	values := make(map[string]string)
	for line := range strings.Lines(string(encoded)) {
		line = strings.TrimSpace(line)
		if line == "" || strings.HasPrefix(line, "#") {
			continue
		}
		name, value, exists := strings.Cut(line, "=")
		if !exists {
			continue
		}
		name = strings.TrimSpace(name)
		if name == "" {
			continue
		}
		values[name] = strings.TrimSpace(value)
	}
	scope := &environmentScope{}
	for name, value := range values {
		_, present := os.LookupEnv(name)
		if present && name != "SANDBOX_EGRESS_PROXY" {
			continue
		}
		if err := scope.set(name, value); err != nil {
			return nil, errors.Join(err, scope.Close())
		}
	}
	if proxy := os.Getenv("SANDBOX_EGRESS_PROXY"); proxy != "" {
		if err := scope.set("HTTPS_PROXY", proxy); err != nil {
			return nil, errors.Join(err, scope.Close())
		}
	}
	return scope, nil
}

func (scope *environmentScope) set(name, value string) error {
	previous, present := os.LookupEnv(name)
	if err := os.Setenv(name, value); err != nil {
		return fmt.Errorf("set environment variable %q: %w", name, err)
	}
	scope.changes = append(scope.changes, environmentChange{
		name: name, value: previous, present: present,
	})
	return nil
}

// unset removes a variable until Close restores it.
func (scope *environmentScope) unset(name string) error {
	previous, present := os.LookupEnv(name)
	if !present {
		return nil
	}
	if err := os.Unsetenv(name); err != nil {
		return fmt.Errorf("unset environment variable %q: %w", name, err)
	}
	scope.changes = append(scope.changes, environmentChange{name: name, value: previous, present: true})
	return nil
}

func (scope *environmentScope) Close() error {
	var closeErr error
	for _, change := range slices.Backward(scope.changes) {
		var err error
		if change.present {
			err = os.Setenv(change.name, change.value)
		} else {
			err = os.Unsetenv(change.name)
		}
		if err != nil {
			closeErr = errors.Join(
				closeErr,
				fmt.Errorf("restore environment variable %q: %w", change.name, err),
			)
		}
	}
	scope.changes = nil
	return closeErr
}

func validateRequest(parsed Request) ([]RequestMessage, error) {
	if parsed.SessionID != nil && strings.TrimSpace(*parsed.SessionID) == "" {
		return nil, errors.New("session_id must not be empty")
	}
	if parsed.ThinkingLevel != "" {
		switch parsed.ThinkingLevel {
		case "low", "medium", "high", "xhigh", "max":
		default:
			return nil, errors.New("thinking_level must be one of: low, medium, high, xhigh, max")
		}
	}
	for _, name := range append(parsed.ExtraAllowedTools, parsed.DisallowedTools...) {
		if strings.TrimSpace(name) == "" {
			return nil, errors.New("tool names must not be empty")
		}
	}
	if parsed.CompactInstructions != "" && !parsed.Compact {
		return nil, errors.New("compact_instructions needs compact")
	}
	if parsed.Compact && parsed.SessionID == nil {
		return nil, errors.New("compact needs the session_id of the session to compact")
	}

	if parsed.Messages == nil {
		if parsed.Prompt == nil {
			if parsed.Compact {
				return nil, nil
			}
			return nil, errors.New("messages must be set")
		}
		return []RequestMessage{{Content: *parsed.Prompt}}, nil
	}
	if len(parsed.Messages) == 0 {
		return nil, errors.New("messages must not be empty")
	}
	for index, message := range parsed.Messages {
		if message.Role != "" && message.Role != "user" {
			return nil, fmt.Errorf("messages[%d].role must be user", index)
		}
		if message.MessageID != nil && strings.TrimSpace(*message.MessageID) == "" {
			return nil, fmt.Errorf("messages[%d].message_id must not be empty", index)
		}
		if message.MessageID != nil {
			if _, err := uuid.Parse(strings.TrimSpace(*message.MessageID)); err != nil {
				return nil, fmt.Errorf("messages[%d].message_id must be a UUID", index)
			}
		}
		if _, err := messageImages(message.Images); err != nil {
			return nil, fmt.Errorf("messages[%d].images: %w", index, err)
		}
	}
	return parsed.Messages, nil
}

func reasoningEffort(level string) llm.ReasoningEffort {
	switch level {
	case "low":
		return llm.ReasoningEffortLow
	case "medium":
		return llm.ReasoningEffortMedium
	case "xhigh":
		return llm.ReasoningEffortXHigh
	case "max":
		return llm.ReasoningEffortMax
	default:
		return llm.ReasoningEffortHigh
	}
}

// openSession resumes the requested session, or creates it unless it must
// exist.
func openSession(
	ctx context.Context,
	store *localfile.Store,
	requested *string,
	mustExist bool,
) (session.ID, sessionstore.ResumeState, error) {
	id := session.ID(uuid.New().String())
	if requested != nil {
		id = session.ID(strings.TrimSpace(*requested))
	}
	restored, err := store.Resume(ctx, id)
	if err == nil {
		return id, restored, nil
	}
	if !errors.Is(err, fs.ErrNotExist) {
		return "", sessionstore.ResumeState{}, fmt.Errorf("open session %q: %w", id, err)
	}
	if mustExist {
		return "", sessionstore.ResumeState{}, fmt.Errorf("session %q does not exist, so there is nothing to compact", id)
	}
	snapshot, err := store.Create(ctx, id)
	if err != nil {
		return "", sessionstore.ResumeState{}, fmt.Errorf("create session %q: %w", id, err)
	}
	return id, sessionstore.ResumeState{Snapshot: snapshot}, nil
}

func (observer *sessionObserver) Observe(sessionID session.ID, item sessionstore.Item) {
	if sessionID != observer.sessionID {
		return
	}
	observer.mu.Lock()
	defer observer.mu.Unlock()
	if observer.err != nil {
		return
	}
	if err := writeSessionItem(observer.output, item); err != nil {
		observer.fail(err)
		return
	}
}

func writeSessionItem(output io.Writer, item sessionstore.Item) error {
	encoded, err := json.Marshal(item)
	if err != nil {
		return fmt.Errorf("encode session item %d: %w", item.Sequence, err)
	}
	if _, err := fmt.Fprintf(output, "%s\n", encoded); err != nil {
		return fmt.Errorf("write session item %d: %w", item.Sequence, err)
	}
	return nil
}

func (observer *sessionObserver) fail(err error) {
	observer.err = err
	observer.cancel()
}

func (observer *sessionObserver) Err() error {
	observer.mu.Lock()
	defer observer.mu.Unlock()
	return observer.err
}

// compactionLimits resolves the model's context window and the request size,
// in tokens, from which the conversation is compacted automatically; 0 turns
// that off. The window comes from KOU_CONVEYOR_CONTEXT_WINDOW, the
// provider's knowledge of the model or defaultContextWindow, and
// KOU_CONVEYOR_AUTO_COMPACT sets the threshold: off, a number of tokens, or
// a percentage of the window.
func compactionLimits(getenv func(string) string, provider Provider, model string) (int64, int64, error) {
	var window int64
	if value := strings.TrimSpace(getenv(contextWindowEnvironment)); value != "" {
		parsed, ok := parseTokens(value)
		if !ok {
			return 0, 0, fmt.Errorf("%s must be a positive number of tokens, such as 200000 or 200k", contextWindowEnvironment)
		}
		window = parsed
	} else if provider.ContextWindow != nil {
		window = provider.ContextWindow(model)
	}
	if window <= 0 {
		window = defaultContextWindow
	}
	setting := strings.ToLower(strings.TrimSpace(getenv(autoCompactEnvironment)))
	switch setting {
	case "":
		return window, window - min(autoCompactReserve, window/4), nil
	case "off", "false", "no", "0":
		return window, 0, nil
	}
	if value, ok := strings.CutSuffix(setting, "%"); ok {
		percent, err := strconv.ParseFloat(strings.TrimSpace(value), 64)
		if err == nil && percent > 0 && percent <= 100 {
			return window, int64(float64(window) * percent / 100), nil
		}
	} else if tokens, ok := parseTokens(setting); ok {
		return window, tokens, nil
	}
	return 0, 0, fmt.Errorf("%s must be off, a number of tokens such as 150000 or 150k, or a share of the context window such as 80%%", autoCompactEnvironment)
}

// parseTokens reads a positive token count, optionally in thousands (k) or
// millions (m).
func parseTokens(value string) (int64, bool) {
	value = strings.ToLower(strings.TrimSpace(value))
	scale := 1.0
	switch {
	case strings.HasSuffix(value, "k"):
		value, scale = strings.TrimSuffix(value, "k"), 1e3
	case strings.HasSuffix(value, "m"):
		value, scale = strings.TrimSuffix(value, "m"), 1e6
	}
	parsed, err := strconv.ParseFloat(strings.TrimSpace(value), 64)
	if err != nil || parsed <= 0 || parsed*scale >= 1<<53 {
		return 0, false
	}
	tokens := int64(parsed * scale)
	return tokens, tokens > 0
}

// steerRun hands the messages that follow the request on stdin to the
// running agent, until stdin ends, stops being JSON or the run ends. The
// agent reads each after the tool calls it is making: a message never cuts a
// response short, and follows the results it waited for. One the run ends
// before is not recorded; its sender learns so from the session.
func steerRun(ctx context.Context, messages *jsontext.Decoder, inputs inbox.Writer, report io.Writer) {
	for {
		value, err := messages.ReadValue()
		if err != nil {
			if !errors.Is(err, io.EOF) && ctx.Err() == nil {
				fmt.Fprintf(report, "steering stopped: %v\n", err)
			}
			return
		}
		input, err := steeringInput(value)
		if err != nil {
			fmt.Fprintf(report, "steering message rejected: %v\n", err)
			continue
		}
		if err := inputs.Submit(ctx, input); err != nil {
			return // the run is over
		}
	}
}

// steeringInput is the input for a message sent while the agent runs.
func steeringInput(value jsontext.Value) (inbox.Input, error) {
	var message RequestMessage
	if err := json.Unmarshal(value, &message, json.RejectUnknownMembers(true)); err != nil {
		return inbox.Input{}, fmt.Errorf("invalid JSON: %w", err)
	}
	if message.Role != "" && message.Role != "user" {
		return inbox.Input{}, errors.New("role must be user")
	}
	if strings.TrimSpace(message.Content) == "" && len(message.Images) == 0 {
		return inbox.Input{}, errors.New("content is empty")
	}
	id := uuid.New().String()
	if message.MessageID != nil {
		id = strings.TrimSpace(*message.MessageID)
		if _, err := uuid.Parse(id); err != nil {
			return inbox.Input{}, errors.New("message_id must be a UUID")
		}
	}
	payload, err := messagePayload(message)
	if err != nil {
		return inbox.Input{}, fmt.Errorf("images: %w", err)
	}
	return inbox.Input{ID: inbox.ID(id), Kind: inbox.InputExternal, Payload: payload, Delivery: inbox.DeliverAfterTools}, nil
}

// lockedWriter serializes the writes of a goroutine that may outlive its
// writer: once closed, it drops them.
type lockedWriter struct {
	mu     sync.Mutex
	w      io.Writer
	closed bool
}

func (w *lockedWriter) Write(p []byte) (int, error) {
	w.mu.Lock()
	defer w.mu.Unlock()
	if w.closed {
		return len(p), nil
	}
	return w.w.Write(p)
}

func (w *lockedWriter) Close() {
	w.mu.Lock()
	w.closed = true
	w.mu.Unlock()
}
