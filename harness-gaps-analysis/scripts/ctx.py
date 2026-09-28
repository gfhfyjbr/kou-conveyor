import json, glob, os, collections
files = sorted(glob.glob('.harness/sessions/*.session.jsonl'), key=os.path.getsize)
inp = []; per = []
models = collections.Counter()
for f in files:
    s_in=[]; 
    for line in open(f):
        try: rec = json.loads(line)
        except Exception: continue
        if rec.get('type')!='item': continue
        it = rec['data']['Item']
        if it['Kind']=='turn': models[it['Data'].get('Model','?')] += 1
        if it['Kind']=='model_response':
            u = it['Data']['Response'].get('Usage') or {}
            if u.get('InputTokens'): s_in.append(u['InputTokens'])
    if s_in:
        inp += s_in; per.append((os.path.basename(f)[:8], len(s_in), max(s_in), sum(s_in)//len(s_in)))
def q(v,p): v=sorted(v); return v[int(len(v)*p)]
print("requests:", len(inp), "median %d p75 %d p90 %d max %d" % (q(inp,.5), q(inp,.75), q(inp,.9), max(inp)))
print("share of requests over 200k: %.0f%%, over 400k: %.0f%%" % (100*sum(1 for x in inp if x>200000)/len(inp), 100*sum(1 for x in inp if x>400000)/len(inp)))
print("tokens over 200k as share of all input tokens: %.0f%%" % (100*sum(x for x in inp if x>200000)/sum(inp)))
print("models:", models.most_common(6))
print("per session (id, responses, max ctx, mean ctx):"); 
for p in per: print("  ", p)
