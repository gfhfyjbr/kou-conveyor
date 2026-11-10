#!/usr/bin/env python3
"""Responses codec/SSE/HTTP parity. Both adapters contact only their own loopback server."""
import base64
import json
from pathlib import Path
import random
import subprocess

ROOT = Path(__file__).resolve().parents[1]
TARGET = ROOT / "tmp/rust-rewrite-target"


def item(kind, data, **fields):
    return {"Type":kind,"Data":data,**fields}


def cases():
    values = []
    def request(label, inputs=None, **fields):
        payload = {"Action":"request","Request":{"Model":{"ID":"m"},"Input":inputs or []}}
        payload["Request"].update(fields.pop("Request", {}))
        payload.update(fields)
        values.append((label,payload))
    def response(label, output=(), **fields):
        values.append((label,{"Action":"decode","Body":{"id":"r","status":"completed","output":output,**fields}}))
    message = lambda role="user",text="hello",**fields:item("message",{"Role":role,"Text":text,**fields})
    call = lambda id="c",args="{}",**fields:item("tool_call",{"CallID":id,"Name":"f","Arguments":args},**fields)
    result = lambda id="c",parts=None:item("tool_result",{"CallID":id,"Output":[{"Kind":"text","Value":"result"}] if parts is None else parts})
    request("empty")
    request("text",[message()])
    request("phase",[message("assistant","answer",Phase="final_answer")])
    request("identified-assistant",[item("message",{"Role":"assistant","Text":"answer","Phase":"final_answer"},ProviderID="msg")])
    request("identified-empty",[item("message",{"Role":"assistant","Text":""},ProviderID="msg")])
    request("identified-user",[item("message",{"Role":"user","Text":"hello"},ProviderID="msg")])
    request("images",[message(Images=[{"Label":"example","URL":"data:image/png;base64,eA=="}])])
    request("image-only",[message(text="",Images=[{"URL":"https://invalid.example/image"}])])
    request("call-result",[call(),result()])
    request("empty-result",[call(),result(parts=[])])
    request("missing-call",[call(),message(text="continue")])
    request("recorded-later",[call(),message(text="continue"),result()])
    request("parallel-group",[call("a"),call("b"),result("b"),call("d"),message(text="next")])
    request("duplicate-result",[call(),result(),message(text="next"),result()])
    request("late-images",[call(),result(),result(parts=[{"Kind":"image","Value":"data:image/png;base64,eA=="},{"Kind":"text","Value":""},{"Kind":"text","Value":"tail"}])])
    for args in ["", "null", "[]", "{half", "{\"a\":1,\"a\":2}", " \t{\"x\":42}\n", "中"*400]:
        request(f"arguments-{len(values)}",[call(args=args),result(),result()])
    request("function",Request={"Tools":[{"Type":"function","Name":"Code","Description":"Run code","Parameters":{"type":"object","required":["code"],"properties":{"code":{"type":"string"}}}}]})
    request("null-parameters",Request={"Tools":[{"Type":"function","Name":"f"}]})
    request("hosted",Request={"Tools":[{"Type":"hosted","Name":"web_search"}]})
    request("bad-tool",Request={"Tools":[{"Type":"unknown","Name":"f"}]})
    request("bad-hosted",Request={"Tools":[{"Type":"hosted","Name":"browse"}]})
    for effort in ["low","medium","high","xhigh","max","invalid"]:
        request("effort-"+effort,Request={"Model":{"ID":"m","ReasoningEffort":effort,"MaxOutputTokens":1024}})
    request("extensions",CacheKey="cache",Extensions={"provider":{"only":["fixture"]},"prompt_cache_retention":"24h"})
    request("extension-override",Extensions={"stream":False})
    request("optional-extension",Extensions={"max_output_tokens":3})
    request("opaque-reasoning",[item("reasoning",{"Summary":["think"],"Raw":{"type":"reasoning","id":"rs","summary":[],"encrypted_content":"opaque","vendor":{"n":2}}})])
    request("foreign-reasoning",[item("reasoning",{"Raw":{"type":"thinking","signature":"foreign"}})])
    request("null-reasoning",[item("reasoning",{"Raw":None})])
    for index, raw in enumerate([[],["reasoning"],["thinking"],[None],42]):
        request(f"opaque-reasoning-nonobject-{index}",[item("reasoning",{"Raw":raw})])
    request("missing-reasoning",[item("reasoning",{"Summary":["not a provider item"]})])
    request("model-switch",[item("reasoning",{"Raw":{"type":"reasoning","encrypted_content":"other"}},Model="old"),item("message",{"Role":"assistant","Text":"old","Phase":"final_answer"},Model="old",ProviderID="must-not-send"),call(Model="old",ProviderID="call-provider")])
    response("complete",[{"type":"message","id":"msg","phase":"final_answer","content":[{"type":"output_text","text":"line\n中🎉"}]}])
    response("reasoning",[{"type":"reasoning","id":"rs","summary":[{"type":"summary_text","text":"think"}],"encrypted_content":"opaque","vendor":True}])
    response("empty-summary",[{"type":"reasoning","id":"rs","summary":[]}])
    response("tool-call",[{"type":"function_call","id":"fc","call_id":"c","name":"f","arguments":"{\"n\":1}","status":"completed"}])
    response("partial-tool-call",[{"type":"function_call","id":"fc","call_id":"c","name":"f","arguments":"{half","status":"in_progress"}])
    response("refusal",[{"type":"message","content":[{"type":"refusal","refusal":"no"}]}])
    response("usage",usage={"input_tokens":20,"input_tokens_details":{"cached_tokens":8,"cache_write_tokens":3},"output_tokens":10,"output_tokens_details":{"reasoning_tokens":4},"vendor":{"n":9}})
    response("null-usage",usage=None)
    response("null-output",output=None)
    response("null-item",[None])
    response("incomplete-limit",status="incomplete",incomplete_details={"reason":"max_output_tokens"})
    response("incomplete-filter",status="incomplete",incomplete_details={"reason":"content_filter"})
    response("failed",status="failed",error={"code":"server_error","message":"no"})
    response("failed-no-error",status="failed",error=None)
    response("unknown-status",status="queued")
    for action in [{"type":"search","query":"one"},{"type":"search","queries":["a","b"]},{"type":"open_page","url":"https://invalid.example/"},{"type":"find_in_page","url":"https://invalid.example/","pattern":"\"word\""}]:
        response("web-"+str(len(values)),[{"id":"ws","type":"web_search_call","status":"completed","action":action}])
    response("xai-extra",instructions="",max_tool_calls=2,presence_penalty=0.5)
    rng = random.Random(9917)
    for n in range(45):
        inputs = []
        for k in range(rng.randrange(1,5)):
            id = f"c{k}"
            inputs.append(call(id,args=json.dumps({"n":n,"text":"中\n"},ensure_ascii=False)))
            if rng.randrange(2): inputs.append(result(id))
            if rng.randrange(3) == 0: inputs.append(message(text="next"))
        request(f"generated-{n}",inputs)
    values.extend(stream_cases())
    values.extend(http_cases())
    values.extend(codex_cases())
    return values


def stream_cases():
    def wire(*events, ending="\n"):
        return "".join("data: " + json.dumps(event,ensure_ascii=False,separators=(",",":")) + ending + ending for event in events)
    def terminal(output=None, **fields):
        return {"type":"response.completed","response":{"id":"r","status":"completed","output":output or [],**fields}}
    def message(text):
        return {"id":text,"type":"message","content":[{"type":"output_text","text":text}]}
    done = lambda n,text:{"type":"response.output_item.done","output_index":n,"item":message(text)}
    scenarios = [
        ("terminal",wire(terminal([message("中🎉")]))),
        ("ordered-fallback",wire(done(3,"last"),done(0,"first"),terminal(usage={"input_tokens":10}))),
        ("duplicate-item",wire(done(0,"old"),done(0,"new"),terminal())),
        ("terminal-wins",wire(done(0,"old"),terminal([message("authoritative")]))),
        ("late-ignore",wire(terminal([message("done")]),done(1,"late"))),
        ("reasoning-delta",wire({"type":"response.reasoning_summary_text.delta","output_index":0,"delta":"think"},{"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":1,"delta":"not first"},{"type":"response.reasoning_text.delta","output_index":2,"delta":"reason"},terminal())),
        ("text-delta",wire({"type":"response.output_text.delta","output_index":3,"delta":"line\n中"},{"type":"response.output_text.delta","output_index":3,"delta":""},terminal([message("line\n中")]))),
        ("delta-without-index",wire({"type":"response.output_text.delta","delta":"ignore"},terminal())),
        ("invalid-delta",wire({"type":"response.output_text.delta","output_index":0,"delta":42},terminal())),
        ("nested-error",wire({"type":"error","error":{"code":"denied","message":"no","type":"permission_error"}})),
        ("flat-error",wire({"type":"error","code":"failed","message":"reason","param":"input"})),
        ("no-terminal",wire(done(0,"orphan"))),
        ("missing-body",wire({"type":"response.completed"})),
        ("null-body",wire({"type":"response.completed","response":None})),
        ("done-only","data: [DONE]\n\n"),
        ("partial-terminal",wire(terminal())[:-1]),
        ("comments-bom","\ufeff:comment\r\n\r\n"+wire(terminal([message("crlf")]),ending="\r\n")),
        ("bare-cr",wire(done(0,"cr"),terminal(),ending="\r")),
        ("multiline",'data: {"type":"response.completed",\ndata: "response":{"id":"r","status":"completed","output":[]}}\n\n'),
        ("empty-data","data:\n\n"+wire(terminal())),
        ("null-item",wire({"type":"response.output_item.done","output_index":0,"item":None},terminal())),
        ("negative-index",wire({"type":"response.output_item.done","output_index":-1,"item":message("ignore")},terminal())),
        ("scalar-item",wire({"type":"response.output_item.done","output_index":0,"item":42},terminal())),
        ("summary-paragraph",wire({"type":"response.reasoning_summary_part.added","output_index":0,"summary_index":0},{"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":0,"delta":"first"},{"type":"response.reasoning_summary_part.added","output_index":0,"summary_index":1},{"type":"response.reasoning_summary_text.delta","output_index":0,"summary_index":1,"delta":"second"},terminal())),
    ]
    return [(f"stream-{label}-chunk{chunk}",{"Action":"stream","Stream":value,"ChunkSize":chunk}) for label,value in scenarios for chunk in (0,1,7)]


def http_cases():
    values = []
    def wire(*events):
        return "".join("data: " + json.dumps(event,ensure_ascii=False,separators=(",",":")) + "\n\n" for event in events)
    success = {"Status":200,"Body":wire({"type":"response.completed","response":{"id":"r","status":"completed","output":[]}})}
    def failure(code="server_error", message="bad", kind="", status=503, **fields):
        return {"Status":status,"Body":json.dumps({"error":{"code":code,"message":message,"type":kind,**fields}},ensure_ascii=False,separators=(",",":"))}
    def stream_error(code, message="bad", kind="", terminal=False, **fields):
        error = {"code":code,"message":message,"type":kind,**fields}
        event = {"type":"response.failed","response":{"id":"failed","status":"failed","output":[],"error":error}} if terminal else {"type":"error","error":error}
        return {"Status":200,"Body":wire(event)}
    def case(label, replies, request=None, attempts=1, repeat=1, family="", cache_ttl="", **fields):
        values.append(("http-"+label,{"Action":"http","HTTP":{"Replies":replies,"MaxAttempts":attempts,"Repeat":repeat,"Family":family,"CacheTTL":cache_ttl},"Request":request or {"Model":{"ID":"m"}},**fields}))
    case("success",[success],CacheKey="session 中")
    for status in [307,400,401,403,408,425,429,500,502,503,504,520,521,522,523,524,529,599]:
        case(f"status-{status}",[failure(status=status)])
    final_codes = ["context_length_exceeded","insufficient_quota","usage_not_included","usage_limit_reached","credit_balance_exhausted","billing_hard_limit_reached","cyber_policy","misalignment_policy_violation","invalid_prompt","bio_policy","invalid_api_key","invalid_token"]
    for code in final_codes:
        for transport in ["http","error","failed"]:
            reply = failure(code,status=429) if transport == "http" else stream_error(code,terminal=transport=="failed")
            case(f"final-{transport}-{code}",[reply,success],attempts=2)
    for kind in ["authentication_error","permission_error","insufficient_quota","usage_limit_reached"]:
        case("type-"+kind,[failure(code="",kind=kind,status=429),success],attempts=2)
    for index, body in enumerate(["", " plain text \n", "null", "[]", "42", '{"error":null}', '{"error":["invalid_api_key","bad"]}', '{"error":["server_error","message"]}', '[["server_error","message"]]', '{"error":{"type":"only-type"}}', '{"error":{"code":42,"message":"typed"}}', '{"error":{"message":42}}', '{"error":{"code":"a","code":"b","message":"duplicate"}}', '{"error":{"message":"escaped\\n中"}}']):
        case(f"error-body-{index}",[{"Status":400,"Body":body}])
    for index, reset in enumerate([1600000000,1600000000.9,"2020-09-13T12:26:40Z","2020-09-13T15:26:40+03:00",None,0,-1,"invalid"]):
        case(f"reset-http-{index}",[failure(code="usage_limit_reached",status=429,resets_at=reset)])
        case(f"reset-stream-{index}",[stream_error("usage_limit_reached",resets_at=reset)])
    for index, invalid in enumerate([None,0,"invalid"]):
        case(f"nested-reset-fallback-{index}",[{"Status":200,"Body":wire({"type":"error","resets_at":invalid,"error":{"code":"usage_limit_reached","message":"bad","resets_at":1600000000}})}])
    for code in ["rate_limit_exceeded","usage_limit_reached","insufficient_quota"]:
        reply = failure(code,"try again in 28ms",status=429)
        reply["RetryAfter"] = "120"
        case("hint-"+code,[reply])
    for index, message in enumerate(["try again in 1.3333333333s", "try again in 0.0000000006s", "try again in 1.23456789ms", "попробуйtry again in 1s", "try again in 1s中文", "try again in\u00a01s", "try again in 1\u00a0s", "try again in ١s", "try again in 999999999999999999999s"]):
        case(f"message-hint-{index}",[failure("usage_limit_reached",message,status=429)])
    context_message = "Maximum context length is 128000 tokens, however you requested about 130225 tokens."
    for transport in ["http","error","failed"]:
        reply = failure("context_length_exceeded",context_message,status=400) if transport == "http" else stream_error("context_length_exceeded",context_message,terminal=transport=="failed")
        case("overflow-"+transport,[reply])
    case("overflow-message-only",[failure("invalid_request_error",context_message,status=400)])
    case("stream-eof",[{"Status":200,"Body":"data: [DONE]\n\n"}])
    case("error-byte-cap",[{"Status":503,"Body":"x"*((1<<20)+1)}])
    for transport in ["http","error","failed"]:
        reply = failure("rate_limit_exceeded","try again in 1ms",status=503) if transport == "http" else stream_error("rate_limit_exceeded","try again in 1ms",terminal=transport=="failed")
        case("retry-"+transport,[reply,success],attempts=2)
    reply = failure("server_error"); reply["RetryAfter"] = "1"
    case("retry-header",[reply,success],attempts=2)
    case("array-error-still-retries",[{"Status":503,"Body":'{"error":["invalid_api_key","bad"]}',"RetryAfter":"1"},success],attempts=2)
    image_request = {"Model":{"ID":"text-only"},"Input":[
        item("message",{"Role":"user","Text":" look ","Images":[{"Label":" cat ","URL":"data:private"},{"Label":" ","URL":"data:second"}]}),
        item("message",{"Role":"assistant","Text":"","Images":[{"URL":"data:assistant"}]},ProviderID="assistant-id"),
        item("tool_result",{"CallID":"c","Output":[{"Kind":"image","Value":"data:private"},{"Kind":"text","Value":"after"}]}),
        item("tool_result",{"CallID":"c","Output":[{"Kind":"image","Value":"data:late"},{"Kind":"text","Value":"late"}]}),
    ]}
    for transport in ["http","error","failed"]:
        reply = failure("","Images are not supported",status=400) if transport == "http" else stream_error("","Images are not supported",terminal=transport=="failed")
        case("images-"+transport,[reply,success],request=image_request,repeat=2,CacheKey="images")
    case("images-still-rejected",[failure("","Images are not supported",status=400)],request=image_request,repeat=2)
    case("no-image-no-fallback",[failure("","Images are not supported",status=400)])
    for family in ["openai","fireworks","openrouter","ollama","codex"]:
        case(f"{family}-success",[success],family=family,CacheKey="session 中")
        case(f"{family}-auth-final",[failure("invalid_api_key","rejected",status=401)],family=family,attempts=2)
        case(f"{family}-rate-limit",[failure("usage_limit_reached","try again in 28ms",status=429)],family=family)
        case(f"{family}-retry",[failure("rate_limit_exceeded","try again in 1ms"),success],family=family,attempts=2)
        case(f"{family}-images",[failure("","Images are not supported",status=400),success],family=family,request=image_request,repeat=2)
    for cap in [0,1,100]:
        case(f"codex-reject-output-cap-{cap}",[success],family="codex",request={"Model":{"ID":"m","MaxOutputTokens":cap}})
    for ttl in ["5m","1h"," 5M ",""]:
        case("openrouter-ttl-"+ttl,[success],family="openrouter",cache_ttl=ttl,CacheKey="cache")
    usages = [None,{}, {"prompt_tokens":11,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":3},"completion_tokens_details":{"reasoning_tokens":5}},
        {"prompt_tokens":0,"completion_tokens":0,"prompt_tokens_details":{"cached_tokens":0},"completion_tokens_details":{"reasoning_tokens":0}},
        {"input_tokens":3,"output_tokens":7,"input_tokens_details":{"cached_tokens":1},"output_tokens_details":{"reasoning_tokens":2}},
        {"prompt_tokens":None,"completion_tokens":None,"prompt_tokens_details":None,"completion_tokens_details":None},
        {"prompt_tokens_details":{"cached_tokens":None},"completion_tokens_details":{"reasoning_tokens":None}},
        {"prompt_tokens":9223372036854775807,"completion_tokens":-5},
        {"input_tokens":8,"output_tokens":4,"prompt_tokens":11,"completion_tokens":7,"prompt_tokens_details":{"cached_tokens":3},"completion_tokens_details":{"reasoning_tokens":5}},
    ]
    for index, usage in enumerate(usages):
        case(f"fireworks-usage-{index}",[{"Status":200,"Body":wire({"type":"response.completed","response":{"id":"f","status":"completed","output":[],"usage":usage}})}],family="fireworks")
    return values


def codex_cases():
    values = []
    def case(label, **config):
        values.append(("codex-config-"+label,{"Action":"codex-config","Codex":config}))
    def jwt(raw):
        return "header."+base64.urlsafe_b64encode(raw.encode()).decode().rstrip("=")+".signature"
    def claims(account="account", expires=0):
        return jwt(json.dumps({"exp":expires,"https://api.openai.com/auth":{"chatgpt_account_id":account}},separators=(",",":")))
    case("missing")
    for index, (token, account) in enumerate([
        ("opaque","account"),(" opaque "," account "),("opaque",""),("sk-not-a-subscription","account"),
        ("bad token","account"),("bad\r\nheader","account"),("opaque","bad\r\nheader"),("opaque","中"),
        ("one.!.three","account"),("opaque..four.parts","account"),("two.parts","account"),
        ("header.e31.signature","account"),("header.e30=.signature","account"),
        (claims(),""),(claims(),"other"),(claims(expires=1),""),(claims(expires=-1),""),
        (claims(expires=4102444800),""),(claims(account=""),"account"),(claims(account=""),""),
    ]):
        case(f"token-{index}",AccessToken=token,AccountID=account)
    for index, raw in enumerate(["null","[]","{}",'{"exp":null,"https://api.openai.com/auth":null}', '{"exp":"1"}', '{"exp":1.2}',
        '{"exp":1,"exp":2}', '{"https://api.openai.com/auth":[]}', '{"https://api.openai.com/auth":{"chatgpt_account_id":5}}',
        '{"exp":9223372036854775807,"unknown":1e999}', '{"exp":9223372036854775808}', "{broken"]):
        case(f"claims-{index}",AccessToken=jwt(raw),AccountID="account")
    auth = json.dumps({"auth_mode":"chatgpt","tokens":{"access_token":claims("from-file"),"refresh_token":"unused"}},separators=(",",":"))
    case("auth-file",AuthBody=auth)
    case("auth-file-account-mismatch",AuthBody=json.dumps({"tokens":{"access_token":claims(),"account_id":"other"}}))
    case("auth-file-mixed",AuthBody=auth,AccessToken="opaque",AccountID="account")
    case("auth-file-public-mode",AuthBody=auth,FileMode=0o644)
    case("auth-file-too-big",AuthBody="x"*((1<<20)+1))
    for index, body in enumerate(["null","[]","{}",'{"tokens":null}', '{"tokens":[]}', '{"tokens":["secret","account"]}',
        '{"tokens":{"access_token":42}}','{"tokens":{"access_token":null,"account_id":null}}', '{"tokens":{},"tokens":{}}',
        '{"auth_mode":"apikey","tokens":{"access_token":"opaque","account_id":"account"}}',
        '{"auth_mode":null,"tokens":{"access_token":"opaque","account_id":"account"}}',
        '{"tokens":{"access_token":"opaque","account_id":"account"},"unknown":1e999}',"{broken"]):
        case(f"file-schema-{index}",AuthBody=body)
    urls = ["", "https://chatgpt.com/backend-api/codex", " https://chatgpt.com/backend-api/codex/// ", "http://127.0.0.1:1234/prefix", "http://[::1]:1234", "http://[::ffff:127.0.0.1]:1234", "http://127.0.0.1?", "http://127.0.0.1#",
        "https://api.openai.com/v1", "http://chatgpt.com/backend-api/codex", "https://chatgpt.com:443/backend-api/codex", "http://localhost", "http://127.1", "http://2130706433", "http://0x7f000001", "http://127.0.00.1", "http://0.0.0.0", "http://192.168.1.1", "http://user:pass@127.0.0.1", "http://@127.0.0.1", "http://127.0.0.1/?key=value", "http://127.0.0.1/#fragment", "ftp://127.0.0.1", "http://127.0.0.1\\external"]
    for index, url in enumerate(urls):
        case(f"url-{index}",AccessToken="opaque",AccountID="account",BaseURL=url)
    case("zero-attempts",AccessToken="opaque",AccountID="account",MaxAttempts=0)
    case("zero-attempts-missing-token",MaxAttempts=0)
    case("bad-url-missing-token",BaseURL="http://external.invalid")
    case("bad-url-before-file-validation",BaseURL="http://external.invalid",AuthBody="invalid-json")
    environments = [{}, {"OPENAI_API_KEY":"sk-never-read"}, {"OPENAI_CODEX_ACCESS_TOKEN":" token ","OPENAI_CODEX_ACCOUNT_ID":" account "},
        {"OPENAI_CODEX_ACCOUNT_ID":" account "}, {"OPENAI_CODEX_AUTH_FILE":" /fixture/auth.json "},
        {"OPENAI_CODEX_ACCESS_TOKEN":"token","OPENAI_CODEX_AUTH_FILE":"/fixture/auth.json"},
        {"OPENAI_CODEX_ACCOUNT_ID":"account","OPENAI_CODEX_AUTH_FILE":"/fixture/auth.json"},
        {"CODEX_HOME":" /fixture/codex "}, {"CODEX_HOME":"relative/../codex//"}, {"HOME":"/fixture/home/.."}]
    for index, env in enumerate(environments):
        case(f"environment-{index}",Mode="environment",Environment={"HOME":"/fixture/home",**env})
    return values


def execute(command, payload):
    result = subprocess.run(list(map(str,command)),input=json.dumps(payload,ensure_ascii=False).encode(),stdout=subprocess.PIPE,stderr=subprocess.PIPE,cwd=ROOT,timeout=10,check=True)
    return json.loads(result.stdout)


def main():
    subprocess.run(["go","build","-o",TARGET/"kou-provider-oracle","./cmd/kou-provider-oracle"],cwd=ROOT,check=True)
    subprocess.run(["cargo","build","--locked","-p","kou-provider","--example","responses_oracle"],cwd=ROOT,check=True)
    corpus = cases()
    for label, payload in corpus:
        left = execute([TARGET/"kou-provider-oracle"],payload)
        right = execute([TARGET/"debug/examples/responses_oracle"],payload)
        if left != right: raise AssertionError(f"{label}:\nGo: {left!r}\nRust: {right!r}")
    print(f"PASS: {len(corpus)} Responses/client/Codex cases ({len(http_cases())} local HTTP, {len(codex_cases())} synthetic credential/config cases); no production credentials or paid calls")


if __name__ == "__main__":
    main()
