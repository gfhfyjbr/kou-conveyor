import json, glob, re, collections
c = collections.Counter(); parallel_same_file = 0; examples = []
path_re = re.compile(r"[\w./-]+\.(?:go|py|js|ts|tsx|md|json|yaml|yml|css|html|sh|toml)\b")
for f in glob.glob('.harness/sessions/*.session.jsonl'):
    for line in open(f):
        try: rec = json.loads(line)
        except Exception: continue
        if rec.get('type')!='item' or rec['data']['Item']['Kind']!='model_response': continue
        out = rec['data']['Item']['Data']['Response'].get('Output') or []
        cmds = []
        for o in out:
            if o['Type']=='tool_call' and o['Data']['Name']=='Bash':
                try: cmd = json.loads(o['Data']['Arguments']).get('command','')
                except Exception: continue
                cmds.append(cmd)
                for m in re.finditer(r"<<\s*(-?)\s*(['\"]?)(\w+)\2", cmd):
                    quoted = bool(m.group(2)); body = cmd[m.end():]
                    end = body.find('\n'+m.group(3))
                    body = body[:end] if end>=0 else body
                    c['heredoc'] += 1
                    if not quoted:
                        c['heredoc_unquoted'] += 1
                        if re.search(r"\$[\w{(]|`", body): c['heredoc_unquoted_with_dollar_or_backtick'] += 1
        # parallel edits touching the same file in one response
        edits = [cmd for cmd in cmds if re.search(r"<<|sed -i|python3? -c|>\s*\S", cmd)]
        if len(edits) >= 2:
            c['responses_with_parallel_edits'] += 1
            sets = [set(path_re.findall(e)) for e in edits]
            for i in range(len(sets)):
                for j in range(i+1, len(sets)):
                    if sets[i] & sets[j]:
                        parallel_same_file += 1
                        if len(examples) < 3: examples.append(sorted(sets[i] & sets[j])[:3])
                        break
                else: continue
                break
c['responses_with_parallel_edits_touching_same_file'] = parallel_same_file
for k in sorted(c): print(f"{k:55s} {c[k]}")
print(examples)
