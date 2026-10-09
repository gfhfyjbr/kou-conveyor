import json,collections,re,statistics as st,datetime as dt
F="/Users/gfhfyjbr/Projects/Rust/joroweave/.harness/sessions/234b16c9-59b5-4771-9d39-8dc8efb6ce9a.session.jsonl"
turns=[]
ops={}
cur=None
inputs=[]
def ts(s): return dt.datetime.fromisoformat(s.replace('Z','+00:00'))
with open(F) as f:
    for l in f:
        o=json.loads(l)
        if o['type']=='operation':
            op=o['data'].get('Operation')
            if not op: continue
            ops.setdefault(op['ID'],[]).append(op)
            continue
        if o['type']!='item': continue
        it=o['data']['Item']; k=it['Kind']; t=ts(it['RecordedAt'])
        if k=='input': inputs.append((t,it['Data']))
        elif k=='turn':
            cur={'id':it['Data']['ID'],'type':it['Data']['Type'],'start':t,'seq':it['Sequence'],'resp':None,'calls':[],'statuses':[]}
            turns.append(cur)
        elif k=='model_response':
            r=it['Data']['Response']; cur['resp']=r; cur['end']=t
            u=r.get('Usage') or {}
            cur['in']=u.get('InputTokens',0); cur['cached']=u.get('CachedInputTokens',0); cur['cw']=u.get('CacheWriteInputTokens',0)
            cur['out']=u.get('OutputTokens',0); cur['think']=u.get('ReasoningTokens',0)
            for x in r['Output']:
                if x['Type']=='tool_call': cur['calls'].append(x['Data'])
                elif x['Type']=='message': cur.setdefault('msgs',[]).append(x['Data'].get('Text',''))
                elif x['Type']=='reasoning':
                    raw=x['Data'].get('Raw') or {}
                    cur.setdefault('thinktext',[]).append(raw.get('thinking','') if isinstance(raw,dict) else '')
        elif k=='tool_call_status':
            cur['statuses'].append(it['Data'])
print('turns',len(turns),'first',turns[0]['start'],'last',turns[-1]['start'])
print('inputs:')
for t,d in inputs: print(' ',t,d['Kind'],json.dumps(d['Payload'],ensure_ascii=False)[:400])
comps=[t for t in turns if t['type']=='compaction']
print('\ncompactions',len(comps))
prev=None
for i,c in enumerate(comps):
    dur=(c['end']-c['start']).total_seconds() if c.get('resp') else None
    summ=''.join(c.get('msgs',[]))
    m=re.search(r'<summary>(.*?)(</summary>|$)',summ,re.S)
    sl=len(m.group(1)) if m else len(summ)
    gap=(c['start']-prev).total_seconds()/60 if prev else None
    print(f"#{i+1} {c['start'].strftime('%m-%d %H:%M')} dur={dur and round(dur)}s in={c.get('in')} out={c.get('out')} think={c.get('think')} summary_chars={sl} since_prev={gap and round(gap,1)}min")
    prev=c['start']
reg=[t for t in turns if t['type']=='regular' and t.get('resp')]
ncalls=[len(t['calls']) for t in reg]
print('\nregular turns',len(reg),'calls total',sum(ncalls),'mean',round(st.mean(ncalls),2),'single-call share',round(sum(1 for n in ncalls if n==1)/len(ncalls),3),'zero-call',sum(1 for n in ncalls if n==0))
print('calls/turn hist',sorted(collections.Counter(min(n,8) for n in ncalls).items()))
lat=[(t['end']-t['start']).total_seconds() for t in reg]
print('latency s: median',round(st.median(lat),1),'p90',round(sorted(lat)[int(.9*len(lat))],1),'max',round(max(lat)),'sum h',round(sum(lat)/3600,2))
clat=[(t['end']-t['start']).total_seconds() for t in comps if t.get('resp')]
print('compaction latency: median',round(st.median(clat)),'max',round(max(clat)),'sum h',round(sum(clat)/3600,2))
think=[t['think'] for t in reg]; out=[t['out'] for t in reg]
print('think tokens: sum',sum(think),'median',st.median(think),'p90',sorted(think)[int(.9*len(think))],'max',max(think),'>10k',sum(1 for x in think if x>10000))
print('output tokens sum',sum(out),'input sum (incl cached)',sum(t['in'] for t in reg),'cached sum',sum(t['cached'] for t in reg),'cache write sum',sum(t['cw'] for t in reg))
tot=[t['in'] for t in reg]
print('input tokens per turn: median',st.median(tot),'p90',sorted(tot)[int(.9*len(tot))],'max',max(tot))
gaps=[]
for a,b in zip(turns,turns[1:]):
    if a.get('end'): gaps.append(((b['start']-a['end']).total_seconds(),a['end']))
gaps.sort(reverse=True)
print('\nlargest gaps between response and next turn (s, at):')
for g,at in gaps[:15]: print(' ',round(g),at)
print('sum gaps h',round(sum(g for g,_ in gaps)/3600,2))
names=collections.Counter(); errs=collections.Counter(); errex={}
for t in turns:
    for c in t['calls']: names[c['Name']]+=1
    for s in t['statuses']:
        e=s['Status'].get('Error')
        if e:
            key=re.sub(r'"[^"]*"','"…"',e)[:90]; errs[key]+=1; errex.setdefault(key,(t['start'],e[:300]))
print('\ntool names',names)
print('\nvalidation errors',sum(errs.values()))
for k,v in errs.most_common(30): print(v,k,'|',errex[k][0])
msgs=[(t['start'],m) for t in reg for m in t.get('msgs',[]) if m.strip()]
print('\nvisible messages',len(msgs),'median len',st.median(len(m) for _,m in msgs),'max',max(len(m) for _,m in msgs))
idx=[{'i':i,'id':t['id'],'type':t['type'],'start':t['start'].isoformat(),'end':t.get('end') and t['end'].isoformat(),'in':t.get('in'),'cached':t.get('cached'),'cw':t.get('cw'),'out':t.get('out'),'think':t.get('think'),'calls':[{'name':c['Name'],'args':c.get('Arguments',''),'id':c.get('CallID')} for c in t['calls']],'statuses':t['statuses'],'msgs':t.get('msgs',[]),'thinklen':sum(len(x) for x in t.get('thinktext',[])),'thinktext':t.get('thinktext',[]) if t['type']=='regular' else [],'summary':(''.join(t.get('msgs',[])) if t['type']=='compaction' else None)} for i,t in enumerate(turns)]
json.dump(idx,open('/tmp/audit/turns.json','w'))
opsum={}
for oid,vs in ops.items():
    last=vs[-1]
    opsum[oid]={'type':last.get('Type'),'status':last.get('Status'),'n':len(vs),'state':last.get('State'),'first_status':vs[0].get('Status')}
json.dump(opsum,open('/tmp/audit/ops.json','w'))
print('ops',len(opsum), collections.Counter((v['type'],v['status']) for v in opsum.values()))
