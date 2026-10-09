import json, glob, os, collections, datetime, re
files = sorted(glob.glob('.harness/sessions/*.session.jsonl'), key=os.path.getsize)
T = collections.Counter()
lat_model = []; lat_tool = []; stale = []
sleep_re = re.compile(r"\bsleep\s+\d")
heredoc_lines = []
py_replace = 0; py_edit = 0
def ts(s): return datetime.datetime.fromisoformat(s.replace('Z','+00:00'))
for f in files:
    open_calls = {}     # cid -> scheduled time
    completions = {}    # cid -> completion time
    turn_start = {}; turn_type = {}
    pending_turn = None; pending_open = set()
    with open(f) as fh:
        for line in fh:
            try: rec = json.loads(line)
            except Exception: continue
            if rec.get('type') != 'item': continue
            item = rec['data']['Item']; kind = item['Kind']; d = item['Data']; at = ts(item['RecordedAt'])
            if kind == 'turn':
                turn_type[d['ID']] = d.get('Type'); turn_start[d['ID']] = at
                if d.get('Type') == 'regular':
                    pending_turn = d['ID']; pending_open = set(open_calls)
            elif kind == 'tool_call_status':
                cid = d['CallID']; st = d['Status']; ops = rec['data'].get('Operations') or []
                if st.get('Error'): open_calls.pop(cid, None); continue
                terminal = all(o.get('Status') in ('completed','failed','canceled') for o in ops) if ops else False
                if cid not in open_calls and cid not in completions:
                    if terminal: completions[cid] = at
                    else: open_calls[cid] = at
                elif cid in open_calls and terminal:
                    completions[cid] = at; open_calls.pop(cid, None)
                    lat_tool.append((at - completions.get(cid, at)).total_seconds())
            elif kind == 'model_response':
                tid = d['TurnID']
                if turn_type.get(tid) != 'regular': continue
                lat_model.append((at - turn_start[tid]).total_seconds())
                if tid == pending_turn and pending_open:
                    # were all the open calls done before the model answered?
                    done_before = [c for c in pending_open if c in completions and completions[c] <= at]
                    T['placeholder_turns'] += 1
                    if len(done_before) == len(pending_open):
                        T['placeholder_turns_all_done_before_response'] += 1
                        stale.append(max((completions[c]-turn_start[tid]).total_seconds() for c in pending_open))
                    elif done_before:
                        T['placeholder_turns_some_done_before_response'] += 1
                out = d['Response'].get('Output') or []
                for o in out:
                    if o['Type']=='tool_call' and o['Data']['Name']=='Bash':
                        try: cmd = json.loads(o['Data']['Arguments']).get('command','')
                        except Exception: cmd=''
                        if sleep_re.search(cmd): T['bash_with_sleep'] += 1
                        m = re.search(r"<<\s*-?['\"]?(\w+)['\"]?\n(.*?)\n\1\b", cmd, re.S)
                        if m: heredoc_lines.append(m.group(2).count('\n')+1)
                        if re.search(r"python3?\s+-\s*<<|python3?\s+-c", cmd):
                            py_edit += 1
                            if '.replace(' in cmd or 're.sub' in cmd: py_replace += 1
import statistics as S
def q(v,p): 
    v=sorted(v); return v[int(len(v)*p)] if v else None
print("regular responses:", len(lat_model))
print("model latency s: median %.1f p90 %.1f mean %.1f" % (q(lat_model,.5), q(lat_model,.9), S.mean(lat_model)))
for k in sorted(T): print(f"{k:50s} {T[k]}")
print("stale placeholder turns: outstanding call finished within (s) median %.1f p90 %.1f" % (q(stale,.5), q(stale,.9)))
print("heredocs:", len(heredoc_lines), "lines median", q(heredoc_lines,.5), "p90", q(heredoc_lines,.9), "max", max(heredoc_lines))
print("python edits:", py_edit, "of which search/replace:", py_replace)
