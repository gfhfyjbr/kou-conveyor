import json,collections,re,statistics as st,datetime as dt
T=json.load(open('/tmp/audit/turns.json'))
O=json.load(open('/tmp/audit/ops.json'))
def ts(s): return dt.datetime.fromisoformat(s)
# ---------- 1. thinking retention: input delta vs previous output
print("=== 1. context growth per turn (within segments) ===")
deltas=[]; drops=[]
for a,b in zip(T,T[1:]):
    if a['type']!='regular' or b['type']!='regular' or not a.get('in') or not b.get('in'): continue
    d=b['in']-a['in']
    # tool result size estimate for a's calls: sum of op result text length/4
    tool_chars=0
    for s in a['statuses']:
        for oid in s['Status'].get('WaitingFor') or []:
            op=O.get(oid)
            if not op: continue
            stt=op['state'] or {}
            r=stt.get('Result') or {}
            if op['type']=='shell':
                tool_chars+=len((r or {}).get('Out','') or '')+len((r or {}).get('Err','') or '')
            elif op['type']=='file':
                tool_chars+=len((r or {}).get('Text','') or '')
    deltas.append((d,a['out'],a['think'],tool_chars/3.5,b['start']))
    if d< -20000: drops.append((d,a['in'],b['in'],b['start']))
errs_keep=[]; errs_strip=[]
for d,out,think,tool,_ in deltas:
    if tool>60000: continue  # truncated outputs distort
    errs_keep.append(abs(d-(out+tool))); errs_strip.append(abs(d-(out-think+tool)))
print('n',len(errs_keep),'median abs err if thinking KEPT:',round(st.median(errs_keep)),' if STRIPPED:',round(st.median(errs_strip)))
big=[(d,out,think,tool,t) for d,out,think,tool,t in deltas if think>15000 and tool<20000]
print('turns with think>15k & small tool output: (delta_in, out, think, tool_est)')
for x in big[:12]: print('  ',[round(v) if isinstance(v,float) else v for v in x])
print('input drops >20k inside regular turns (pruning evidence):',len(drops))
for x in drops[:20]: print('  ',x)
# ---------- 2. segments between compactions
print("\n=== 2. segments ===")
segs=[]; cur=[]
for t in T:
    if t['type']=='compaction':
        segs.append(cur); cur=[]
    else: cur.append(t)
segs.append(cur)
def is_action(c):
    n=c['name']
    if n in('Edit','Write'): return True
    if n=='Bash':
        a=c['args'].lower()
        return any(k in a for k in ['cargo build','cargo test','cargo run','cargo check','cargo clippy','just ','managed-probe','probe','cfrun','cmpev','docker run','./target'])
    return False
noact=0; firstact=[]
for i,s in enumerate(segs):
    calls=[c for t in s for c in t['calls']]
    acts=[j for j,c in enumerate(calls) if is_action(c)]
    edits=sum(1 for c in calls if c['name'] in('Edit','Write'))
    if not acts: noact+=1
    else:
        firstact.append(acts[0])
    if s:
        dur=(ts(s[-1]['start'])-ts(s[0]['start'])).total_seconds()/60
        print(f"seg{i:02d} {s[0]['start'][5:16]} turns={len(s):3d} calls={len(calls):3d} edits={edits:3d} first_action_call_idx={acts[0] if acts else '-'} dur={dur:.0f}min")
print('segments with no action:',noact,'/',len(segs),' median calls before first action',st.median(firstact))
# ---------- 3. grace_Read origin
print("\n=== 3. hallucinated tool names ===")
for i,t in enumerate(T):
    bad=[c for c in t['calls'] if c['name'] in('grace_Read','away_Bash','brain_Write')]
    if bad:
        print('first bad at turn',i,t['start'],[c['name'] for c in bad])
        # look back for 'grace_Read' string in tool results of prior turns
        for j in range(max(0,i-40),i):
            for s in T[j]['statuses']:
                for oid in s['Status'].get('WaitingFor') or []:
                    op=O.get(oid); 
                    if not op: continue
                    stt=op['state'] or {}; r=stt.get('Result') or {}
                    txt=(r.get('Out','') or '')+(r.get('Text','') or '')
                    if 'grace_Read' in txt or 'away_Bash' in txt:
                        k=txt.find('grace_Read'); print('  seen in result of turn',j,T[j]['start'],op['type'],json.dumps(stt.get('Input') or stt.get('Path'))[:200],'\n    ...',txt[max(0,k-200):k+100].replace('\n','\\n'))
        for j in range(max(0,i-3),i+1):
            for th in T[j].get('thinktext',[]):
                if 'grace' in th: print('  thinking turn',j,':',th[:300].replace('\n',' '))
        break
badturns=[(t['start'],[c['name'] for c in t['calls'] if c['name'] in('grace_Read','away_Bash','brain_Write')]) for t in T if any(c['name'] in('grace_Read','away_Bash','brain_Write') for c in t['calls'])]
print('turns with bad names',len(badturns),'span',badturns[0][0],'..',badturns[-1][0])
# ---------- 4. Edit race
print("\n=== 4. Edit race ===")
edit_by_turn=[]
lost=0; multi_same=0; repeated=[]
seen={}  # (path,old)->turn index
for i,t in enumerate(T):
    paths=collections.Counter()
    for c in t['calls']:
        if c['name']!='Edit': continue
        try: a=json.loads(c['args'])
        except: continue
        p=a.get('path'); old=a.get('old_string','')
        paths[p]+=1
        key=(p,old[:200])
        if key in seen and seen[key][1]:  # earlier edit with same key succeeded
            repeated.append((seen[key][0],i,p))
        # did it succeed?
        ok=None
        for s in t['statuses']:
            if s['CallID']==c['id']:
                for oid in s['Status'].get('WaitingFor') or []:
                    op=O.get(oid)
                    if op: ok=(op['status']=='completed')
        seen[key]=(i,ok)
    if any(v>1 for v in paths.values()): multi_same+=1
print('turns with >1 Edit to same file',multi_same)
print('re-applied identical edits (same path+old_string) after an earlier success:',len(repeated))
for a,b,p in repeated[:10]: print('  turn',a,'->',b,p.split('/')[-1])
# ---------- 5. repeated reads
print("\n=== 5. repeated reads ===")
reads=collections.Counter(); readsegs=collections.defaultdict(set)
segof={}
si=0
for t in T:
    if t['type']=='compaction': si+=1; continue
    for c in t['calls']:
        p=None
        if c['name'] in('Read','grace_Read'):
            try: p=json.loads(c['args']).get('path')
            except: pass
        elif c['name']=='Bash':
            m=re.search(r'(sed -n|cat|rg|grep)[^|;&]*?(/[^\s"\']+\.(rs|cpp|h|md|txt|py|js|toml))',c['args'])
            if m: p=m.group(2)
        if p: reads[p]+=1; readsegs[p].add(si)
print('distinct files read',len(reads),'total reads',sum(reads.values()))
for p,n in reads.most_common(25): print(f'{n:4d} segs={len(readsegs[p]):2d} {p}')
# ---------- 6. shell stats
print("\n=== 6. shell ===")
durs=[]; fails=0; sleeps=0; timeouts=0; bigout=0; builds=[]; probes=0; tests=0
for oid,op in O.items():
    if op['type']!='shell': continue
    s=op['state'] or {}; inp=s.get('Input') or {}; r=s.get('Result') or {}
    cmd=inp.get('Command','')
    if re.search(r'\bsleep\s+\d',cmd): sleeps+=1
    if r.get('ExitCode') not in (0,None): fails+=1
    if 'timed out' in (r.get('Out','') or '')[-300:] or s.get('TimedOut'): timeouts+=1
    if s.get('StartedAt') and s.get('FinishedAt'):
        d=(ts(s['FinishedAt'])-ts(s['StartedAt'])).total_seconds(); durs.append(d)
        if 'cargo build' in cmd or 'cargo test' in cmd: builds.append((d,cmd[:80]))
    if 'managed-probe' in cmd or 'cfrun' in cmd: probes+=1
    if 'cargo test' in cmd: tests+=1
    if len(r.get('Out','') or '')>=39000: bigout+=1
print('shell ops',sum(1 for o in O.values() if o['type']=='shell'),'nonzero exit',fails,'sleep cmds',sleeps,'timeouts',timeouts,'outputs hitting 40k cap',bigout,'probe runs cmds',probes,'cargo test cmds',tests)
if durs: print('durations s: median',round(st.median(durs),1),'p90',round(sorted(durs)[int(.9*len(durs))]),'max',round(max(durs)),'sum h',round(sum(durs)/3600,2))
builds.sort(reverse=True)
print('longest builds/tests:',[(round(d),c) for d,c in builds[:6]])
print('keys in shell state:',list((next(o for o in O.values() if o['type']=='shell')['state']).keys()))
# ---------- 7. Read sizes
print("\n=== 7. Read result sizes ===")
rs=[]
for op in O.values():
    if op['type']=='file' and (op['state'] or {}).get('Action')=='read':
        r=(op['state'] or {}).get('Result') or {}; rs.append(len(r.get('Text','') or ''))
if rs: print('reads',len(rs),'median chars',st.median(rs),'p90',sorted(rs)[int(.9*len(rs))],'max',max(rs),'sum MB',round(sum(rs)/1e6,1))
# ---------- 8. summary composition: hypotheses retention
print("\n=== 8. summaries: section sizes ===")
secs=collections.Counter(); n=0
for t in T:
    if t['type']!='compaction' or not t['summary']: continue
    m=re.search(r'<summary>(.*?)(</summary>|$)',t['summary'],re.S)
    if not m: continue
    body=m.group(1); n+=1
    parts=re.split(r'\n\s*(?:#+\s*)?(\d)\.\s+([A-Z][^\n:]*):?',body)
    # parts: [pre, num, title, text, num, title, text...]
    for k in range(1,len(parts)-2,3):
        secs[parts[k+1].strip()[:40]]+=len(parts[k+2])
tot=sum(secs.values())
for k,v in secs.most_common(12): print(f'{v/tot*100:5.1f}%  {k}')
# ---------- 9. messages to user
print("\n=== 9. visible messages ===")
ms=[(t['start'],m) for t in T if t['type']=='regular' for m in t.get('msgs',[]) if len(m)>400]
print('messages >400 chars:',len(ms))
for s,m in ms[:8]: print(' ',s,len(m),m[:120].replace('\n',' '))
