package codevm

import (
	"context"
	"os"
	"path/filepath"
	"strings"
	"sync"
	"testing"
	"time"
	"unicode/utf8"
)

func testConfig(t *testing.T) Config {
	t.Helper()
	return Config{Shell: "/bin/sh", Directory: t.TempDir(), RunTimeout: 20, CommandTimeout: 10}
}

func TestRunCallsToolsAtOnceAndReturnsAValue(t *testing.T) {
	config := testConfig(t)
	var reports int
	result := Run(t.Context(), config, `
		const [a, b] = await Promise.all([bash('echo one'), bash('echo two; exit 3')]);
		await write('note.txt', 'hello\nworld\n');
		const numbered = await read('note.txt');
		await edit('note.txt', 'world', 'there');
		const text = await readText('note.txt');
		console.log('lines', numbered.split('\n').length);
		return { a: a.stdout.trim(), b: b.exitCode, text, err: b.stderr };
	`, func(Result) { reports++ })
	if result.Error != "" {
		t.Fatalf("error = %q\n%s", result.Error, result.Text())
	}
	if result.Value != "{\n  \"a\": \"one\",\n  \"b\": 3,\n  \"text\": \"hello\\nthere\\n\",\n  \"err\": \"\"\n}" {
		t.Fatalf("value = %q", result.Value)
	}
	if len(result.Calls) != 6 || result.Calls[0].Name != "bash" || result.Calls[2].Name != "write" || result.Calls[4].Name != "edit" {
		t.Fatalf("calls = %+v", result.Calls)
	}
	if code := result.Calls[1].ExitCode; code == nil || *code != 3 {
		t.Fatalf("exit code = %v", code)
	}
	if result.Logs != "lines 2\n" {
		t.Fatalf("logs = %q", result.Logs)
	}
	// The first call is reported at once; the rest, a burst, wait for the
	// interval, and the run ends before it.
	if reports < 1 {
		t.Fatalf("reports = %d", reports)
	}
	// The code logged and returned what it needed: the outputs of the calls
	// that succeeded are left out, those of the one that failed are not.
	text := result.Text()
	for _, want := range []string{"[CODE] 6 tool calls", "- bash: echo one (", "- bash: echo two; exit 3 (exit 3", "Exit code: 3", "- read: ", "(The outputs of the calls that succeeded are left out", "console:\n   lines 2", "Return value:\n{"} {
		if !strings.Contains(text, want) {
			t.Fatalf("text lacks %q:\n%s", want, text)
		}
	}
	for _, unwanted := range []string{"     1\thello", "\n   one\n", "|\n"} {
		if strings.Contains(text, unwanted) {
			t.Fatalf("text holds %q:\n%s", unwanted, text)
		}
	}
	if data, _ := os.ReadFile(filepath.Join(config.Directory, "note.txt")); string(data) != "hello\nthere\n" {
		t.Fatalf("note.txt = %q", data)
	}
}

func TestRunReportsErrorsWithLineNumbers(t *testing.T) {
	config := testConfig(t)
	result := Run(t.Context(), config, "const x = 1;\nthrow new Error('boom');", nil)
	if !strings.Contains(result.Error, "boom") || !strings.Contains(result.Error, ":2:") {
		t.Fatalf("error = %q", result.Error)
	}
	result = Run(t.Context(), config, "const x = ;", nil)
	if !strings.Contains(result.Error, "SyntaxError") {
		t.Fatalf("error = %q", result.Error)
	}
	// A rejected call can be caught; uncaught, it ends the run.
	result = Run(t.Context(), config, `
		let caught = '';
		try { await edit('missing.txt', 'a', 'b'); } catch (e) { caught = e.message; }
		await read('also-missing.txt');
		return 'unreachable';
	`, nil)
	if !strings.Contains(result.Error, "also-missing.txt does not exist") || result.Value != "" {
		t.Fatalf("result = %+v", result)
	}
	if len(result.Calls) != 2 || !strings.Contains(result.Calls[0].Error, "does not exist") {
		t.Fatalf("calls = %+v", result.Calls)
	}
}

func TestRunStopsAtTheTimeoutAndOnCancel(t *testing.T) {
	config := testConfig(t)
	config.RunTimeout = 0.3
	started := time.Now()
	result := Run(t.Context(), config, `await bash('sleep 30'); return 'done';`, nil)
	if !strings.Contains(result.Error, "ran longer than") || time.Since(started) > 10*time.Second {
		t.Fatalf("result = %+v after %s", result, time.Since(started))
	}
	if len(result.Calls) != 1 || result.Calls[0].Running || result.Calls[0].Error == "" {
		t.Fatalf("calls = %+v", result.Calls)
	}
	// A busy loop is interrupted too.
	result = Run(t.Context(), config, `while (true) {}`, nil)
	if !strings.Contains(result.Error, "ran longer than") {
		t.Fatalf("result = %+v", result)
	}
	ctx, cancel := context.WithCancel(t.Context())
	go func() { time.Sleep(100 * time.Millisecond); cancel() }()
	config.RunTimeout = 60
	result = Run(ctx, config, `await sleep(60000);`, nil)
	if !strings.Contains(result.Error, "canceled") {
		t.Fatalf("result = %+v", result)
	}
}

func TestRunCommandTimeoutAndBoundedOutput(t *testing.T) {
	config := testConfig(t)
	config.MaxOutputLength = 100
	result := Run(t.Context(), config, `
		const slow = await bash('sleep 30', { timeout: 0.2 });
		const loud = await bash('yes | head -c 100000');
		return { timedOut: slow.timedOut, len: loud.stdout.length, exit: slow.exitCode };
	`, nil)
	if result.Error != "" {
		t.Fatalf("error = %q", result.Error)
	}
	if !strings.Contains(result.Value, `"timedOut": true`) || strings.Contains(result.Value, `"len": 100000`) {
		t.Fatalf("value = %s", result.Value)
	}
	if !strings.Contains(result.Calls[1].Output, "bytes truncated") {
		t.Fatalf("output = %q", result.Calls[1].Output)
	}
}

func TestRunLimitsCallsAndBindsPluginTools(t *testing.T) {
	config := testConfig(t)
	config.MaxCalls = 2
	config.Skills = map[string]string{"review": filepath.Join(config.Directory, "SKILL.md")}
	os.WriteFile(config.Skills["review"], []byte("Review well."), 0o644)
	config.Tools = []CommandTool{{Name: "echo_args", Description: "Echoes.", Command: []string{"cat"}, Parameters: map[string]any{"type": "object", "properties": map[string]any{"x": map[string]any{"type": "integer"}}}}}
	result := Run(t.Context(), config, `
		const s = await skill('review');
		const echoed = await echo_args({ x: 1 });
		let third = 'ok';
		try { await bash('true'); } catch (e) { third = e.message; }
		return s + '|' + echoed.trim() + '|' + third;
	`, nil)
	if result.Error != "" || result.Value != `Review well.|{"x":1}|the run made 2 tool calls, the most a run may make` {
		t.Fatalf("result = %+v", result)
	}
	declarations := Declarations(config)
	for _, want := range []string{`declare function skill(name: "review")`, "// Echoes.", "declare function echo_args(args: { x?: number }): Promise<string>;", "declare function bash("} {
		if !strings.Contains(declarations, want) {
			t.Fatalf("declarations lack %q:\n%s", want, declarations)
		}
	}
}

func TestTypeOf(t *testing.T) {
	schema := map[string]any{
		"type": "object",
		"properties": map[string]any{
			"name":  map[string]any{"type": "string", "description": "The name."},
			"tags":  map[string]any{"type": "array", "items": map[string]any{"type": "string"}},
			"mode":  map[string]any{"enum": []any{"fast", "slow"}},
			"inner": map[string]any{"type": "object", "properties": map[string]any{"n": map[string]any{"type": "integer"}}, "required": []any{"n"}},
			"a-b":   map[string]any{"type": []any{"string", "null"}},
		},
		"required": []any{"name"},
	}
	want := `{ "a-b"?: string | null; inner?: { n: number }; mode?: "fast" | "slow"; name: string /* The name. */; tags?: string[] }`
	if got := typeOf(schema, 0); got != want {
		t.Fatalf("typeOf = %s\nwant %s", got, want)
	}
}

func TestRunShowsTheOutputsWhenTheCodeReportsNothing(t *testing.T) {
	config := testConfig(t)
	result := Run(t.Context(), config, `await bash('echo shown'); await write('a.txt', 'x');`, nil)
	text := result.Text()
	for _, want := range []string{"- bash: echo shown (", "\n   shown\n", "- write: ", "   Wrote "} {
		if !strings.Contains(text, want) {
			t.Fatalf("text lacks %q:\n%s", want, text)
		}
	}
	// A run that fails shows them too, for what went wrong.
	result = Run(t.Context(), config, "await bash('echo before');\nconsole.log('x');\nthrow new Error('stop');", nil)
	text = result.Text()
	if !strings.Contains(text, "\n   before\n") || !strings.Contains(text, "Error: Error: stop\n\tat <eval>:3:") || strings.Contains(text, "left out") {
		t.Fatalf("text:\n%s", text)
	}
}

func TestRunEndsCodeThatWaitsForNothing(t *testing.T) {
	started := time.Now()
	result := Run(t.Context(), testConfig(t), `await sleep(10); await new Promise(() => {}); return 'never';`, nil)
	if !strings.Contains(result.Error, "nothing will settle") || time.Since(started) > 5*time.Second {
		t.Fatalf("result = %+v after %s", result, time.Since(started))
	}
}

func TestRunShowsValuesAsJavaScriptDoes(t *testing.T) {
	config := testConfig(t)
	result := Run(t.Context(), config, `
		console.log(new Error('x'));
		console.log({ b: 1, a: 2 }, new Map([['k', 1]]), new Set([1]), undefined, null, NaN, [1, 2], 'text', Promise.resolve(1), () => 1);
		const cycle = {}; cycle.self = cycle;
		return cycle;
	`, nil)
	if !strings.HasPrefix(result.Logs, "Error: x\n\tat <eval>:2:") || !strings.Contains(result.Logs, `{"b":1,"a":2} Map [["k",1]] Set [1] undefined null NaN [1,2] text [a Promise: await it for its value] [Function]`) {
		t.Fatalf("logs = %q", result.Logs)
	}
	if result.Error != "" || !strings.HasPrefix(result.Value, "[Object, not JSON: TypeError: Converting circular structure") {
		t.Fatalf("result = %+v", result)
	}
	result = Run(t.Context(), config, `throw { code: 7 };`, nil)
	if result.Error != `uncaught {"code":7}` {
		t.Fatalf("error = %q", result.Error)
	}
	// Code the runtime calls to show the value is stopped with the run.
	config.RunTimeout = 0.3
	result = Run(t.Context(), config, `return { toJSON() { while (true) {} } };`, nil)
	if !strings.Contains(result.Error, "ran longer than") || result.Value != "" {
		t.Fatalf("result = %+v", result)
	}
}

func TestRunChecksTheArguments(t *testing.T) {
	config := testConfig(t)
	os.WriteFile(filepath.Join(config.Directory, "a.txt"), []byte("x y y\n"), 0o644)
	for code, want := range map[string]string{
		`await bash();`:           "bash: the command must be a string, not undefined",
		`await write('b.txt');`:   "write: the content must be a string, not undefined",
		`await read('a.txt', 5);`: "read: the options must be an object, not a number",
		`await edit('a.txt', 'y', 'z', { replace_all: true });`: `edit: unknown option "replace_all"; the options are replaceAll`,
		`await edit('a.txt', 'y', 'z');`:                        "oldString appears 2 times in",
		`await edit('a.txt', '', 'z');`:                         "oldString must not be empty; write() creates",
		`await bash('true', { timeout: 1, maxOutput: 5 });`:     `unknown option "maxOutput"; the options are timeout, maxOutputLength`,
		`await tools.echo([1]);`:                                "echo: the arguments must be an object, not an array",
	} {
		config.Tools = []CommandTool{{Name: "echo", Command: []string{"cat"}}}
		result := Run(t.Context(), config, code, nil)
		if !strings.Contains(result.Error, want) || strings.Contains(result.Error, "(native)") {
			t.Fatalf("%s: error = %q, want %q", code, result.Error, want)
		}
	}
	if _, err := os.Stat(filepath.Join(config.Directory, "b.txt")); !os.IsNotExist(err) {
		t.Fatalf("b.txt: %v", err)
	}
	// Options that are empty, or given as they are declared, work.
	result := Run(t.Context(), config, `await edit('a.txt', 'x', 'w', {}); return await edit('a.txt', 'y', 'z', { replaceAll: true });`, nil)
	if result.Error != "" || !strings.Contains(result.Value, "replaced 2 occurrences, the first at line 1") {
		t.Fatalf("result = %+v", result)
	}
	if data, _ := os.ReadFile(filepath.Join(config.Directory, "a.txt")); string(data) != "w z z\n" {
		t.Fatalf("a.txt = %q", data)
	}
}

func TestRunPluginToolsInheritTheEnvironmentAndRejectFailures(t *testing.T) {
	t.Setenv("KOU_CODEVM_TEST", "inherited")
	config := testConfig(t)
	config.CallID = "call-7"
	config.Tools = []CommandTool{
		{Name: "env_tool", Command: []string{"sh", "-c", "echo $KOU_CODEVM_TEST $KOU_CONVEYOR_TOOL_NAME $KOU_CONVEYOR_TOOL_CALL_ID"}},
		{Name: "failing", Command: []string{"sh", "-c", "echo out; exit 4"}},
		{Name: "read", Command: []string{"cat"}},
		{Name: "delete", Command: []string{"cat"}},
	}
	os.WriteFile(filepath.Join(config.Directory, "a.txt"), []byte("one\n"), 0o644)
	result := Run(t.Context(), config, `
		const env = await env_tool({});
		let failure = '';
		try { await failing({}); } catch (e) { failure = e.message; }
		const echoed = await tools.read({ a: 1 });
		return [env.trim(), failure, echoed.trim(), await read('a.txt'), typeof tools.delete].join('|');
	`, nil)
	if result.Error != "" || result.Value != "inherited env_tool call-7-0|failing failed:\nout\n\nExit code: 4|{\"a\":1}|     1\tone|function" {
		t.Fatalf("result = %+v", result)
	}
	if code := result.Calls[1].ExitCode; code == nil || *code != 4 || !strings.Contains(result.Calls[1].Error, "failing failed") {
		t.Fatalf("calls = %+v", result.Calls)
	}
	// The tools named like the runtime's functions or JavaScript's words and
	// globals are declared under tools[] only.
	config.Tools = append(config.Tools, CommandTool{Name: "JSON"}, CommandTool{Name: "a-b"})
	declarations := Declarations(config)
	for _, want := range []string{`// tools["read"](args: `, `// tools["delete"](args: `, `// tools["JSON"](args: `, `// tools["a-b"](args: `, "declare function env_tool(args: ", "rejects when it fails"} {
		if !strings.Contains(declarations, want) {
			t.Fatalf("declarations lack %q:\n%s", want, declarations)
		}
	}
	if strings.Count(declarations, "declare function read(") != 1 || strings.Contains(declarations, "declare function delete(") {
		t.Fatalf("declarations:\n%s", declarations)
	}
}

func TestRunKeepsTheRecordBoundedAndTheValuesWhole(t *testing.T) {
	config := testConfig(t)
	config.MaxOutputLength = 100_000
	result := Run(t.Context(), config, `const r = await bash("head -c 50000 /dev/zero | tr '\\0' x"); return r.stdout.length;`, nil)
	if result.Error != "" || result.Value != "50000" {
		t.Fatalf("result = %+v", result)
	}
	if call := result.Calls[0]; !call.Truncated || len(call.Output) > recordLimit+100 || !strings.Contains(call.Output, "bytes left out") {
		t.Fatalf("output of %d bytes, truncated %v", len(call.Output), call.Truncated)
	}
}

func TestRunReportsAtIntervalsWithoutImages(t *testing.T) {
	config := testConfig(t)
	config.ViewImage = func(context.Context, string) (ImageResult, error) {
		return ImageResult{DataURL: "data:image/png;base64,AAAA", Description: "1x1 image/png"}, nil
	}
	var mu sync.Mutex
	var reports []Result
	result := Run(t.Context(), config, `await viewImage('a.png'); await sleep(700); await bash('true'); return 'ok';`, func(progress Result) {
		mu.Lock()
		defer mu.Unlock()
		reports = append(reports, progress)
	})
	mu.Lock()
	defer mu.Unlock()
	if result.Error != "" || len(result.Images()) != 1 {
		t.Fatalf("result = %+v", result)
	}
	if len(reports) < 2 || len(reports) > 4 {
		t.Fatalf("%d reports", len(reports))
	}
	for _, report := range reports {
		if len(report.Images()) != 0 {
			t.Fatalf("a report carries an image: %+v", report)
		}
	}
}

func TestRunSurvivesAToolThatPanics(t *testing.T) {
	config := testConfig(t)
	config.ViewImage = func(context.Context, string) (ImageResult, error) { panic("boom") }
	result := Run(t.Context(), config, `let message = ''; try { await viewImage('a.png'); } catch (e) { message = e.message; } return message;`, nil)
	if result.Error != "" || result.Value != "internal error: boom" {
		t.Fatalf("result = %+v", result)
	}
}

func TestClipKeepsRunesWhole(t *testing.T) {
	clipped := clip(strings.Repeat("я", 100), 51)
	if !utf8.ValidString(clipped) || !strings.Contains(clipped, "bytes left out") {
		t.Fatalf("clipped = %q", clipped)
	}
	if line := firstLine(strings.Repeat("я", 100)); !utf8.ValidString(line) {
		t.Fatalf("line = %q", line)
	}
	patch := Call{Name: "applyPatch", Arguments: "*** Begin Patch\n*** Update File: a.go\n@@\n-x\n+y\n*** Add File: b.go\n+z\n*** End Patch"}
	if line := argumentLine(patch); line != "Update a.go, Add b.go" {
		t.Fatalf("line = %q", line)
	}
}
