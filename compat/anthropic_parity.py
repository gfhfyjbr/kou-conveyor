#!/usr/bin/env python3
"""Anthropic adapter parity against the current Go SDK, with synthetic replies only."""
import json
import random
import subprocess
from provider_parity import ROOT, TARGET, execute, item


def cases():
    values = []
    def message(role="user", text="hello", **fields):
        return item("message", {"Role": role, "Text": text, **fields})
    def call(id="c", args="{}"):
        return item("tool_call", {"CallID": id, "Name": "Code", "Arguments": args})
    def result(id="c", output=None):
        return item("tool_result", {"CallID": id, "Output": [{"Kind": "text", "Value": "result"}] if output is None else output})
    def thinking(raw=None, **fields):
        return item("reasoning", {"Raw": {"type": "thinking", "signature": "sig", "thinking": "think"} if raw is None else raw}, **fields)
    def request(label, inputs=None, model="claude-opus-5", settings=None, **fields):
        values.append((label, {"Action": "anthropic-request", "Anthropic": settings or {}, "Request": {"Model": {"ID": model}, "Input": [message()] if inputs is None else inputs, **fields}}))
    request("simple")
    request("official", settings={"Official": True})
    request("empty", [])
    request("system-only", [message("system", "prompt")])
    request("blank", [message(text=" \n\t")])
    request("unknown-role", [message("developer")])
    request("alternating", [message("system", "s1"), message("system", "s2"), message(), message(text="joined"), message("assistant", "a"), message("system", "later"), message(text="next")])
    request("images", [message(Images=[{"Label": "example", "URL": "data:image/png;base64,eA=="}, {"URL": "https://image.invalid/x"}])])
    for index, url in enumerate(["data:image/jpg;base64,eA==", "data:IMAGE/GIF;base64,", "data:image/webp;base64,eB==", "data:image/png;base64,eA==\r\n", "data:image/png;base64,eA", "data:image/svg+xml;base64,eA==", "data:image/png;BASE64,eA==", "data:image/png;base64;extra,eA==", "data:image/png;base64,e A==", "data:image/png;base64,!!!!", "http://", "HTTPS://image.invalid", "relative.png"]):
        request(f"image-{index}", [message(text="", Images=[{"Label": " \n", "URL": url}])])
    for inputs in [[call()], [call(), result()], [call(), result(output=[])], [call("a"), call("b"), result("b"), message(text="next"), result("a")], [call(), message("assistant", "continuing"), message(text="next")], [call(), result(), result()], [result("unknown")], [thinking(), call(), result(), message(text="next")], [thinking(Model="other"), message()], [thinking({"type": "thinking_reset"}), message()], [thinking(), message(), thinking({"type": "thinking_reset"}), thinking({"type": "redacted_thinking", "data": "opaque"}), message()]]:
        request(f"conversation-{len(values)}", inputs)
    for raw in [None, [], ["thinking", "sig", "text"], {}, {"type": "reasoning", "signature": "sig"}, {"type": "thinking"}, {"type": "thinking", "signature": "sig", "thinking": "line\n中", "extra": True}, {"type": "redacted_thinking", "data": "opaque", "vendor": 1}]:
        request(f"replayed-{len(values)}", [item("reasoning", {"Raw": raw}), message()])
    for args in ["", "null", "[]", "{half", '{"a":1,"a":2}', ' { "b":2, "a":1 } \n', '{"text":"<&>\u2028中"}', "中" * 400]:
        request(f"arguments-{len(values)}", [call(args=args), result(), result()])
    request("tool", Tools=[{"Type": "function", "Name": "Code", "Description": "Run code", "Parameters": {"type": "array", "required": ["code"], "properties": {"z": {"type": "number"}, "code": {"type": "string"}}}}])
    request("tool-official", settings={"Official": True}, Tools=[{"Type": "function", "Name": "f"}])
    request("bad-tool", Tools=[{"Type": "hosted", "Name": "web_search"}])
    for model in ["m", "", "  claude-opus-5 ", "claude-fable-5", "claude-mythos-5", "claude-mythos-preview", "claude-sonnet-5", "claude-opus-4.8", "claude-opus-4-7", "claude-opus-4-6", "anthropic/claude-sonnet-4.6", "claude-3-haiku", "claude-3-5-sonnet", "claude-opus-4.0", "claude-opus-4-1", "claude-opus-4-2", "claude-sonnet-4", "claude-future"]:
        for effort in ["", "low", "high", "xhigh", "max"]:
            request(f"model-{model}-{effort}", model=model, Model={"ID": model, "ReasoningEffort": effort})
    for effort in ["invalid", "HIGH"]:
        request("effort-" + effort, Model={"ReasoningEffort": effort})
    for cap in [None, -1, 0, 1, 1024, 1365, 1366, 8000, 9223372036854775807]:
        request(f"cap-{cap}", Model={"ID": "claude-sonnet-4", "ReasoningEffort": "max", "MaxOutputTokens": cap})
    for settings in [{"CacheTTL": "1h"}, {"CacheTTL": " 5M "}, {"CacheTTL": "invalid"}, {"ThinkingTurns": 0}, {"ThinkingTurns": -1}, {"MaxTokens": -1}, {"MaxTokens": 8192}, {"MaxAttempts": 0}, {"APIKey": " "}, {"ServerFallbacks": True}, {"Official": True, "ServerFallbacks": False}, {"Official": True, "CacheTTL": "1h", "ThinkingTurns": 0}]:
        request(f"config-{len(values)}", settings=settings)
    rng = random.Random(541)
    for n in range(35):
        inputs = [message("system", "prompt")]
        for k in range(rng.randrange(1, 7)):
            inputs.append(call(f"c{k}", args=json.dumps({"text": "line\n中", "n": n})))
            if rng.randrange(2): inputs.append(result(f"c{k}"))
            if rng.randrange(2): inputs.append(message(text="next"))
            if rng.randrange(3) == 0: inputs.append(thinking())
        request(f"generated-{n}", inputs)
    return values


def response_cases():
    values = []
    def response(label, content=(), **fields):
        values.append(("decode-" + label, {"Action": "anthropic-decode", "Anthropic": {}, "Request": {"Input": [item("message", {"Role": "user", "Text": "hello"})]}, "Body": {"id": "msg", "role": "assistant", "type": "message", "content": content, "stop_reason": "end_turn", **fields}}))
    response("empty")
    response("text", [{"type": "text", "text": "line\n中"}, {"type": "text", "text": " "}])
    for content in [[{"type": "thinking", "thinking": "think", "signature": "sig", "extra": 1}], [{"type": "thinking", "thinking": " " ,"signature": "sig"}], [{"type": "thinking", "thinking": "no signature"}], [{"type": "redacted_thinking", "data": "opaque"}], [{"type": "redacted_thinking"}], [{"type": "tool_use", "id": "c", "name": "f", "input": {"b": 2, "a": 1}}], [{"type": "tool_use", "id": "c", "name": "f"}], [{"type": "tool_use", "input": None}], [{"type": "vendor", "opaque": True}]]:
        response(f"blocks-{len(values)}", content)
    blocks = [{"type": "text", "text": "before"}, {"type": "thinking", "thinking": "old", "signature": "sig"}, {"type": "tool_use", "id": "old", "name": "f", "input": {}}, {"type": "fallback", "from": {"model": "m"}, "to": {"model": "new"}}, {"type": "thinking", "signature": "new", "thinking": "new"}, {"type": "text", "text": "after"}, {"type": "tool_use", "id": "new", "name": "f", "input": {}}]
    for stop in ["end_turn", "max_tokens", "refusal", "model_context_window_exceeded", "pause_turn", "unknown", None]:
        response("stop-" + str(stop), blocks, stop_reason=stop)
    for usage in [None, {}, {"input_tokens": 3, "cache_read_input_tokens": 7, "cache_creation_input_tokens": 5, "output_tokens": 8, "output_tokens_details": {"thinking_tokens": 4}, "vendor": {"opaque": 2}}, {"input_tokens": None, "output_tokens_details": None}, {"input_tokens": 9223372036854775807, "cache_read_input_tokens": 1}]:
        response(f"usage-{len(values)}", usage=usage)
    return values


def event(kind, value=None, **fields):
    if value is None:
        value = {"type": kind, **fields}
    return f"event: {kind}\ndata: {json.dumps(value, ensure_ascii=False, separators=(',', ':'))}\n\n"


def stream_cases():
    start = event("message_start", message={"id": "msg", "content": [], "usage": {"input_tokens": 3, "cache_read_input_tokens": 2, "cache_creation_input_tokens": 1, "output_tokens": 0}})
    text = event("content_block_start", index=0, content_block={"type": "text", "text": ""}) + event("content_block_delta", index=0, delta={"type": "text_delta", "text": "line\n中"}) + event("content_block_stop", index=0)
    thought = event("content_block_start", index=1, content_block={"type": "thinking", "thinking": ""}) + event("content_block_delta", index=1, delta={"type": "thinking_delta", "thinking": "think"}) + event("content_block_delta", index=1, delta={"type": "signature_delta", "signature": "sig"}) + event("content_block_stop", index=1)
    tool = event("content_block_start", index=2, content_block={"type": "tool_use", "id": "c", "name": "f", "input": {}}) + event("content_block_delta", index=2, delta={"type": "input_json_delta", "partial_json": '{ "b":2,'}) + event("content_block_delta", index=2, delta={"type": "input_json_delta", "partial_json": '"a":1 }'}) + event("content_block_stop", index=2)
    delta = event("message_delta", delta={"stop_reason": "end_turn"}, usage={"output_tokens": 7, "output_tokens_details": {"thinking_tokens": 4}})
    stop = event("message_stop")
    full = start + text + thought + tool + delta + stop
    rows = [("full", full), ("empty", stop), ("no-start", text + stop), ("truncated", start + text), ("no-dispatch", start + text + stop[:-1]), ("ping", event("ping", {"type": "ping"}) + full), ("unknown-event", "event: vendor\ndata: not-json\n\n" + full), ("bom", "\ufeff" + full), ("crlf", full.replace("\n", "\r\n")), ("cr-only", full.replace("\n", "\r")), ("unknown-delta", start + event("content_block_start", index=0, content_block={"type": "text", "text": "old"}) + event("content_block_delta", index=0, delta={"type": "future", "text": "ignored"}) + stop), ("bad-start-index", start + event("content_block_start", index=9, content_block={"type": "text"})), ("negative-delta", start + event("content_block_delta", index=-1, delta={"type": "text_delta", "text": "bad"})), ("bad-stop-index", start + event("content_block_stop", index=0)), ("in-band-error", start + event("error", error={"type": "invalid_request_error", "message": "bad request"})), ("error-after-stop", full + event("error", error={"type": "invalid_request_error", "message": "late error"})), ("root-type-different", event("completion", {"type": "message_stop"}))]
    for args in ["", "{half", "null", "[]", "{}", '{"a":1,"a":2}']:
        for reason in ["end_turn", "max_tokens", "refusal"]:
            rows.append((f"arguments-{args}-{reason}", start + event("content_block_start", index=0, content_block={"type": "tool_use", "id": "c", "name": "f", "input": {}}) + event("content_block_delta", index=0, delta={"type": "input_json_delta", "partial_json": args}) + event("content_block_stop", index=0) + event("message_delta", delta={"stop_reason": reason}, usage={"output_tokens": 1}) + stop))
    for usage in [{}, {"input_tokens": 9}, {"cache_creation_input_tokens": 8, "cache_read_input_tokens": 10, "input_tokens": 6}, {"input_tokens": None, "cache_read_input_tokens": None, "output_tokens": None}, {"output_tokens": 3, "output_tokens_details": {"thinking_tokens": 9}}, {"output_tokens_details": {"thinking_tokens": None, "future": True}}, {"vendor": {"nested": [1, 2]}}]:
        rows.append((f"usage-{len(rows)}", start + delta + event("message_delta", delta={"stop_reason": "end_turn"}, usage=usage) + stop))
    for fragments in [["{}", '{"a":1,"a":2}'], ["{half", '{"a":1}']]:
        raw = start + event("content_block_start", index=0, content_block={"type": "tool_use", "id": "c", "name": "f", "input": {}})
        for fragment in fragments:
            raw += event("content_block_delta", index=0, delta={"type": "input_json_delta", "partial_json": fragment})
        rows.append(("sdk-repair-" + str(len(rows)), raw + stop))
    for usage in [{}, {"input_tokens": None}, {"vendor": None}, {"vendor": {"nested": 1}}]:
        rows.append(("no-start-usage-" + str(len(rows)), event("message_start", message={"id": "msg"}) + event("message_delta", delta={"stop_reason": "end_turn"}, usage=usage) + stop))
    values = []
    for label, stream in rows:
        for chunk in ([0, 1, 7] if label in {"full", "crlf", "cr-only", "truncated"} else [0]):
            values.append((f"stream-{label}-{chunk}", {"Action": "anthropic-stream", "Anthropic": {}, "Request": {"Input": [item("message", {"Role": "user", "Text": "hello"})]}, "Stream": stream, "ChunkSize": chunk}))
    return values


def http_cases():
    success = event("message_start", message={"id": "msg", "content": [{"type": "text", "text": "ok"}], "stop_reason": "end_turn", "usage": {"input_tokens": 3, "output_tokens": 2}}) + event("message_stop")
    values = []
    def reply(status=200, message="bad request", kind="invalid_request_error", body=None, headers=None):
        return {"Status": status, "Body": success if status == 200 and body is None else json.dumps({"error": {"type": kind, "message": message}}, separators=(',', ':')) if body is None else body, "Headers": headers or {}}
    def case(label, replies, **settings):
        values.append((f"http-{label}", {"Action": "anthropic-http", "Request": {"Model": {"ID": "claude-opus-5"}, "Input": [item("message", {"Role": "user", "Text": "hello"}), item("reasoning", {"Raw": {"type": "thinking", "thinking": "old", "signature": "sig"}}), item("message", {"Role": "assistant", "Text": "old answer"}), item("message", {"Role": "user", "Text": "next"})]}, "Anthropic": {"Replies": replies, **settings}}))
    case("success", [reply()])
    case("retry", [reply(503, headers={"retry-after-ms": "1"}), reply()], MaxAttempts=2)
    case("thinking", [reply(400, "Invalid thinking signature"), reply()])
    case("signature-alone", [reply(400, "invalid signature")])
    case("thinking-alone", [reply(400, "invalid thinking")])
    for part in ["context_management", "context-management", "clear_thinking"]:
        case("disable-" + part, [reply(400, "unsupported " + part), reply()], Repeat=2)
    case("disable-fallback", [reply(400, "unsupported fallback"), reply()], ServerFallbacks=True, Repeat=2)
    case("room", [reply(400, "input length and `max_tokens` exceed context limit: 188240 + 64000 > 200000"), reply()])
    case("mixed-recovery", [reply(400, "invalid thinking signature"), reply(400, "unsupported context_management"), reply(400, "unsupported fallback"), reply(400, "input length and `max_tokens` exceed context limit: 188240 + 64000 > 200000"), reply()], ServerFallbacks=True)
    for status, kind in [(400, "invalid_request_error"), (401, "authentication_error"), (403, "permission_error"), (404, "not_found_error"), (408, "timeout_error"), (409, "api_error"), (413, "request_too_large"), (429, "rate_limit_error"), (500, "api_error"), (529, "overloaded_error"), (400, "overloaded_error")]:
        case(f"status-{status}-{kind}", [reply(status, kind=kind, headers={"request-id": "fixture-request"})])
    for message in ["prompt is too long: 215000 tokens > 200000 maximum", "INPUT IS TOO LONG", "prompt too long", "context window exceeded", "input length and max_tokens exceed context limit: 199000 + 64000 > 200000", "max_tokens: 195904 + 64000 > 200000", "max_tokens: 195905 + 64000 > 200000", "max_tokens: 200000 + 64000 > 200000", "max_tokens: 300000 + 64000 > 200000"]:
        case(f"overflow-{len(values)}", [reply(400, message)])
    for body in ["gateway down", "", "null", "[]", '{"message":"flat message"}', '{"error":{"type":"invalid_request_error"}}', '{"error":["invalid_request_error","wrong"]}', '{"error":{"type":"invalid_request_error","message":[]}}']:
        case(f"raw-error-{len(values)}", [reply(400, body=body)])
    for headers in [{}, {"Retry-After": "3600"}, {"retry-after-ms": "0.0012349", "Retry-After": "3600"}, {"Retry-After": " 1.0000000009 "}, {"anthropic-ratelimit-unified-status": "REJECTED", "anthropic-ratelimit-unified-reset": "1"}, {"anthropic-ratelimit-unified-status": "rejected", "anthropic-ratelimit-unified-reset": "2020-01-01T00:00:00Z"}, {"anthropic-ratelimit-unified-status": "allowed", "anthropic-ratelimit-unified-reset": "4102444800"}]:
        case(f"rate-{len(values)}", [reply(429, kind="rate_limit_error", headers=headers)])
    for status in [400, 404, 408, 413, 414, 422, 429, 499, 500, 529]:
        case(f"empty-error-{status}", [reply(status, body="")])
    case("rate-retry", [reply(429, kind="rate_limit_error", headers={"retry-after-ms": "1"}), reply()], MaxAttempts=2)
    case("rate-exhausted", [reply(429, kind="rate_limit_error", headers={"retry-after-ms": "1"})], MaxAttempts=2)
    return values


def model_cases():
    values = []
    pages = [[], [{"id": "m"}], [{"id": ""}, {"id": "m"}, {"id": "m"}], [None]]
    for index, data in enumerate(pages):
        values.append((f"models-page-{index}", {"Action": "anthropic-models", "Anthropic": {"Replies": [{"Body": json.dumps({"data": data, "has_more": False})}]}}))
    for more in [None, True, False]:
        values.append((f"models-paging-{more}", {"Action": "anthropic-models", "Anthropic": {"Replies": [{"Body": json.dumps({"data": [{"id": "first"}], "last_id": "cursor +/中", "has_more": more})}, {"Body": '{"data":[{"id":"second"}],"has_more":false}'}]}}))
    values.append(("models-empty-with-cursor", {"Action": "anthropic-models", "Anthropic": {"Replies": [{"Body": '{"data":[],"has_more":true,"last_id":"ignored"}'}]}}))
    values.append(("models-no-cursor", {"Action": "anthropic-models", "Anthropic": {"Replies": [{"Body": '{"data":[{"id":"first"}],"has_more":true}'}]}}))
    for status in [404, 405, 401, 500]:
        values.append((f"models-error-{status}", {"Action": "anthropic-models", "Anthropic": {"MaxAttempts": 3, "Replies": [{"Status": status, "Body": '{"error":{"type":"api_error","message":"listing failed"}}'}]}}))
    values.append(("models-late-unavailable", {"Action": "anthropic-models", "Anthropic": {"Replies": [{"Body": '{"data":[{"id":"first"}],"last_id":"cursor"}'}, {"Status": 404, "Body": 'missing'}]}}))
    for content_type in ["application/json", "application/problem+json; charset=utf-8", "text/plain", "text/event-stream", "application/json; invalid"]:
        values.append((f"models-media-{content_type}", {"Action": "anthropic-models", "Anthropic": {"Replies": [{"Body": '{"data":[{"id":"m"}]}', "Headers": {"Content-Type": content_type}}]}}))
    return values


def main():
    subprocess.run(["go", "build", "-o", TARGET / "kou-provider-oracle", "./cmd/kou-provider-oracle"], cwd=ROOT, check=True)
    subprocess.run(["cargo", "build", "--locked", "-p", "kou-provider", "--example", "responses_oracle"], cwd=ROOT, check=True)
    corpus = cases() + response_cases() + stream_cases() + http_cases() + model_cases()
    for label, payload in corpus:
        left = execute([TARGET / "kou-provider-oracle"], payload)
        right = execute([TARGET / "debug/examples/responses_oracle"], payload)
        if left != right:
            raise AssertionError(f"{label}:\nGo: {left!r}\nRust: {right!r}")
    print(f"PASS: {len(corpus)} Anthropic comparisons ({len(cases()) + len(response_cases())} codec, {len(stream_cases())} SSE, {len(http_cases()) + len(model_cases())} owned-loopback HTTP including model listings)")


if __name__ == "__main__":
    main()
