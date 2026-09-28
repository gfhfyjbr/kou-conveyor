import json, glob, os, collections
files = sorted(glob.glob('.harness/sessions/*.session.jsonl'), key=os.path.getsize)
T = collections.Counter()
for f in files:
    open_calls = {}   # callID -> turn scheduled
    completed_since_turn = 0
    turn_type = {}
    last_turn = None
    saw_placeholder_turn = False
    prev_response_after_placeholder = False
    with open(f) as fh:
        for line in fh:
            try: rec = json.loads(line)
            except Exception: continue
            if rec.get('type') != 'item': continue
            item = rec['data']['Item']; kind = item['Kind']; d = item['Data']
            if kind == 'turn':
                turn_type[d['ID']] = d.get('Type')
                if d.get('Type') != 'regular': continue
                T['turns'] += 1
                if open_calls:
                    T['turns_with_running_placeholders'] += 1
                    T['placeholders_shown'] += len(open_calls)
                    saw_placeholder_turn = True
                else:
                    saw_placeholder_turn = False
                completed_since_turn = 0
            elif kind == 'tool_call_status':
                cid = d['CallID']; st = d['Status']
                ops = rec['data'].get('Operations') or []
                waiting = st.get('WaitingFor') or []
                if st.get('Error'):
                    open_calls.pop(cid, None); continue
                if cid not in open_calls and waiting:
                    # first status: scheduled. Are its operations already terminal in this record?
                    terminal = all(o.get('Status') in ('completed','failed','canceled') for o in ops) if ops else False
                    if terminal: T['calls_finished_at_schedule'] += 1
                    else: open_calls[cid] = True
                else:
                    # completion record
                    if cid in open_calls:
                        terminal = all(o.get('Status') in ('completed','failed','canceled') for o in ops) if ops else True
                        if terminal:
                            open_calls.pop(cid, None); completed_since_turn += 1
            elif kind == 'model_response':
                r = d['Response']; out = r.get('Output') or []
                if turn_type.get(d['TurnID']) != 'regular': continue
                calls = [o for o in out if o['Type']=='tool_call']
                text = ' '.join(o['Data'].get('Text','') for o in out if o['Type']=='message')
                if saw_placeholder_turn:
                    T['responses_after_placeholder'] += 1
                    if not calls:
                        T['responses_after_placeholder_no_calls'] += 1
                        if len(text) < 300: T['responses_after_placeholder_short_wait'] += 1
                    u = r.get('Usage') or {}
                    T['placeholder_resp_output_tokens'] += u.get('OutputTokens',0)
                    T['placeholder_resp_input_tokens'] += u.get('InputTokens',0)
                    T['placeholder_resp_cached_tokens'] += u.get('CachedInputTokens',0)
for k in sorted(T): print(f"{k:45s} {T[k]}")
