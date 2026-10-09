import json, glob, collections, re
c = collections.Counter(); samples = []
for f in glob.glob('.harness/sessions/*.session.jsonl'):
    for line in open(f):
        try: rec = json.loads(line)
        except Exception: continue
        if rec.get('type') != 'operation': continue
        op = rec['data']['Operation']
        if op.get('Type') != 'shell' or op.get('Status') != 'completed': continue
        err = ((op.get('State') or {}).get('Result') or {}).get('Err') or ''
        for m in re.finditer(r"zsh:\d*:? ?([a-z ]+?)(:|$)", err, re.M):
            c[m.group(1).strip()] += 1
        if 'zsh:' in err and len(samples) < 12:
            samples.append(err.strip().split('\n')[0][:160])
print(c.most_common(15)); print('\n'.join(samples))
