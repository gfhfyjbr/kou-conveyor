import json, sys, re, glob, collections, os

files = sorted(glob.glob('.harness/sessions/*.session.jsonl'), key=os.path.getsize)
tot = collections.Counter()
per_session = []
heredoc_re = re.compile(r"<<\s*-?\s*['\"]?[A-Za-z_]+")
edit_patterns = {
  'heredoc_write': re.compile(r"cat\s*>[^|]*<<|<<\s*-?['\"]?EOF"),
  'sed_i': re.compile(r"\bsed\s+-i"),
  'python_edit': re.compile(r"python3?\s+-\s*<<|python3?\s+-c"),
  'perl_i': re.compile(r"\bperl\s+-[a-z]*i"),
  'apply_patch_like': re.compile(r"git apply|patch\s+-p"),
  'printf_write': re.compile(r"printf .*>\s*\S"),
  'echo_write': re.compile(r"echo .*>\s*\S"),
}
read_patterns = {
  'cat_n': re.compile(r"\bcat\s+-n\b"),
  'sed_n': re.compile(r"\bsed\s+-n\b"),
  'grep': re.compile(r"\b(grep|rg)\b"),
  'find': re.compile(r"\bfind\b"),
  'go_test': re.compile(r"\bgo (test|build|vet)\b"),
}
for f in files:
    c = collections.Counter()
    call_names = collections.Counter()
    turn_ids = {}
    wakeups_with_only_running = 0
    responses = 0
    empty_responses = 0
    stops = collections.Counter()
    input_tokens = 0; cached = 0; output_tokens = 0
    compactions = 0
    zsh_errors = 0
    truncated = 0
    with open(f) as fh:
        for line in fh:
            try:
                rec = json.loads(line)
            except Exception:
                continue
            if rec.get('type') != 'item':
                continue
            item = rec['data']['Item']
            kind = item['Kind']
            d = item['Data']
            if kind == 'turn':
                turn_ids[d['ID']] = d.get('Type')
                if d.get('Type') == 'compaction': compactions += 1
            elif kind == 'model_response':
                responses += 1
                r = d['Response']
                stops[r.get('Stop')] += 1
                u = r.get('Usage') or {}
                input_tokens += u.get('InputTokens', 0); cached += u.get('CachedInputTokens', 0); output_tokens += u.get('OutputTokens', 0)
                out = r.get('Output') or []
                calls = [o for o in out if o['Type'] == 'tool_call']
                msgs = [o for o in out if o['Type'] == 'message']
                if not out: empty_responses += 1
                c['calls'] += len(calls)
                c['responses_with_calls'] += 1 if calls else 0
                c['responses_text_only'] += 1 if (msgs and not calls) else 0
                c['parallel_calls_responses'] += 1 if len(calls) > 1 else 0
                for o in calls:
                    call_names[o['Data']['Name']] += 1
                    if o['Data']['Name'] == 'Bash':
                        try:
                            cmd = json.loads(o['Data']['Arguments']).get('command', '')
                        except Exception:
                            cmd = ''
                        for k, rx in edit_patterns.items():
                            if rx.search(cmd): c['edit:'+k] += 1
                        for k, rx in read_patterns.items():
                            if rx.search(cmd): c['read:'+k] += 1
                        if '\n' in cmd: c['multiline_cmd'] += 1
                        c['bash_cmd_chars'] += len(cmd)
            elif kind == 'tool_call_status':
                pass
            elif kind == 'input':
                if d.get('Kind') == 'control':
                    try:
                        cm = json.loads(json.dumps(d['Payload']))
                        c['control:'+cm.get('Mode','?')] += 1
                    except Exception: pass
                elif d.get('Kind') == 'external':
                    c['user_messages'] += 1
    # Pass 2 through operations for outputs: zsh errors & truncation & running placeholders
    with open(f) as fh:
        for line in fh:
            try: rec = json.loads(line)
            except Exception: continue
            if rec.get('type') == 'operation':
                op = rec['data']['Operation']
                if op.get('Type') == 'shell' and op.get('Status') in ('completed',):
                    st = op.get('State') or {}
                    res = st.get('Result') or {}
                    err = res.get('Err') or ''
                    if 'zsh:' in err: zsh_errors += 1
                    if st.get('OutTruncated') or st.get('ErrTruncated'): truncated += 1
                    if res.get('ExitCode', 0) != 0: c['nonzero_exit'] += 1
    per_session.append((os.path.basename(f), os.path.getsize(f), responses, dict(c), dict(call_names), dict(stops), compactions, zsh_errors, truncated, empty_responses, input_tokens, cached, output_tokens))
    tot.update(c); tot['responses'] += responses; tot['compactions'] += compactions; tot['zsh_errors'] += zsh_errors; tot['truncated'] += truncated; tot['empty_responses'] += empty_responses
    tot['input_tokens'] += input_tokens; tot['cached'] += cached; tot['output_tokens'] += output_tokens
    for k,v in stops.items(): tot['stop:'+str(k)] += v
    for k,v in call_names.items(): tot['tool:'+k] += v

print("SESSIONS:", len(files))
for k in sorted(tot): print(f"{k:32s} {tot[k]}")
