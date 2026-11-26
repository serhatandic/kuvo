// Usage preferences through the resident's real service routes, with only isolated accounts.
import assert from 'node:assert/strict';
import path from 'node:path';
import { test } from 'node:test';
import { bench } from './bench.mjs';

const route = 'kuvo/agents/api/kuvo/usage/settings';
const isolated = folder => ({ CODEX_HOME: path.join(folder, 'codex'), GROK_HOME: path.join(folder, 'grok'), CLAUDE_CONFIG_DIR: path.join(folder, 'claude'), KUVO_TEST_PRICE_FEEDS: 'off', KUVO_TEST_PATH_ONLY: '1' });

test('usage settings survive restart and stale changes do not replace them', async t => {
  const { computer } = await bench(t);
  const box = await computer('Usage', undefined, isolated);
  let resident = box.start();
  let call = box.connect(await resident.ready());
  const initial = await call('GET', route);
  assert.equal(initial.status, 200, initial.text);
  assert.deepEqual(initial.json.data.settings, {});
  const setting = { show: true, period: 'today' };
  const changed = await call('PUT', route, { totalSpend: setting });
  assert.equal(changed.status, 200, changed.text);
  const version = changed.json.data.versions.totalSpend;
  assert.ok(version);
  assert.deepEqual(changed.json.data.settings.totalSpend, setting);
  const stale = await call('PUT', route, { totalSpend: {show: false}, based: {totalSpend: null} });
  assert.equal(stale.status, 200, stale.text);
  assert.deepEqual(stale.json.data.stale, ['totalSpend']);
  assert.deepEqual(stale.json.data.settings.totalSpend, setting);
  await resident.stop();
  resident = box.start();
  call = box.connect(await resident.ready());
  const saved = await call('GET', route);
  assert.equal(saved.status, 200, saved.text);
  assert.deepEqual(saved.json.data.settings.totalSpend, setting);
  assert.equal(saved.json.data.versions.totalSpend, version);
});

import { boxBinary, request, until, write } from './adapter.mjs';
import { scriptedCodex } from './codex.mjs';

test('a stale Codex window cannot replace the current ring (Rust correction)', async t => {
  if (boxBinary().typescript) return t.skip('The stale window correction is Rust-only.');
  const codex = await scriptedCodex();
  t.after(() => codex.close());
  // Real adapter credential parsing, with a made-up JWT containing only fixture identifiers.
  const jwt = ['e30', Buffer.from(JSON.stringify({'https://api.openai.com/auth': {chatgpt_account_id:'fixture',chatgpt_user_id:'fixture-user'}})).toString('base64url'), 'fixture'].join('.');
  write(path.join(codex.root,'.codex','auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:'fixture'}}));
  const listener=await codex.start();
  const reset=Math.floor(Date.now()/1000)+7200;
  const sample=async(used,at)=>{
    write(path.join(codex.root,'codex-limits.json'),JSON.stringify({rateLimits:{primary:{usedPercent:used,windowDurationMins:300,resetsAt:at}}}));
    const answer=await request(listener,'POST','kuvo/agents/api/kuvo/limits/refresh',{});
    assert.equal(answer.status,200,answer.text);
    const data=await until(async()=>{
      const reply=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
      return !reply.json.data.refreshPending&&reply.json.data;
    },'The native reading did not settle');
    return {ring:data.rows.find(row=>row.provider==='codex-cli')?.rings.short,notices:data.notices};
  };
  const first=await sample(17,reset);
  assert.equal(first.ring.used,0.17);
  assert.equal(first.ring.resetsAt,reset*1000);
  const stale=await sample(99,reset-3600);
  assert.deepEqual(stale.ring,first.ring);
  const falseRefill=await sample(0,reset+7200);
  assert.deepEqual(falseRefill.ring,first.ring);
  const accepted=await sample(18,reset);
  assert.equal(accepted.ring.used,0.18);
  assert.equal(accepted.ring.resetsAt,reset*1000);
  assert.equal(accepted.notices.length,0);
});

async function noticeFixture(t) {
  const codex=await scriptedCodex();
  t.after(()=>codex.close());
  const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:'fixture',chatgpt_user_id:'fixture-user'}})).toString('base64url'),'fixture'].join('.');
  write(path.join(codex.root,'.codex','auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:'fixture'}}));
  const listener=await codex.start();
  const id='ses_usage_notices';
  codex.seed(id);
  const settings=await request(listener,'PUT',route,{notifications:{underTenPercent:true,healthyToClose:true,closeToRunningOut:true}});
  assert.equal(settings.status,200,settings.text);
  const reset=Math.floor(Date.now()/1000)+7200;
  const read=async tail=>{
    const answer=await request(listener,'GET',tail??'kuvo/agents/api/kuvo/limits');
    assert.equal(answer.status,200,answer.text);
    return answer.json;
  };
  let turns=0;
  const sample=async(used,at=reset)=>{
    const text='sample '+ ++turns;
    codex.script(text,[['account/rateLimits/updated',{rateLimits:{primary:{usedPercent:used,windowDurationMins:300,resetsAt:at}}}]]);
    await codex.prompt(id,text);
    await codex.idles(id, turns);
    return until(async()=>{
      const data=(await read()).data;
      const ring=data.rows.find(row=>row.provider==='codex-cli')?.rings.short;
      return ring?.used===used/100 && data;
    },'The accepted sample did not reach the limits route');
  };
  const setting=async value=>{
    const response=await request(listener,'PUT',route,value);assert.equal(response.status,200,response.text);
  };
  return {read,sample,reset,setting,codex};
}
test('a crossing of ten percent remaining is told only once in its current window (Rust correction)',async t=>{
  if(boxBinary().typescript)return t.skip('Repeated crossing correction is Rust-only.');
  const {sample,reset}=await noticeFixture(t);
  await sample(89);
  const crossed=await sample(91);
  assert.equal(crossed.notices.filter(n=>n.milestone==='underTenPercent').length,1);
  await sample(90,reset+60);
  const repeated=await sample(92,reset+120);
  assert.deepEqual(repeated.notices.map(n=>n.id),crossed.notices.map(n=>n.id));
});
test('ordinary turn readings stay live without rewriting the cache, while new warnings are durable immediately',async t=>{
  if(boxBinary().typescript)return t.skip('The fixture reads the Rust accepted-window journal.');
  const fs=await import('node:fs');
  const {sample,codex}=await noticeFixture(t);
  await sample(89);
  const file=path.join(codex.home,'agents','usage','accepted-limits.json');
  const first=fs.readFileSync(file);
  assert.equal((await sample(90)).rows.find(r=>r.provider==='codex-cli').rings.short.used,0.90);
  assert.deepEqual(fs.readFileSync(file),first,'Ordinary live turns do not rewrite the retained cache within 30 seconds');
  const crossing=await sample(91);const fact=crossing.notices.find(n=>n.milestone==='underTenPercent');assert.ok(fact);
  const saved=JSON.parse(fs.readFileSync(file,'utf8'));
  assert.ok(Object.values(saved.entries).some(entry=>entry.notices?.some(n=>n.id===fact.id)));
});
test('usage warnings arrive through the existing in-app notice history (Rust correction)',async t=>{
  if(boxBinary().typescript)return t.skip('The resident notice correction is Rust-only.');
  const {sample,read}=await noticeFixture(t);
  await sample(89);
  const crossed=await sample(91);
  const fact=crossed.notices.find(n=>n.milestone==='underTenPercent');
  assert.ok(fact);
  const notice=await until(async()=>{
    const data=await read('kuvo/notices');
    return data.notices.find(n=>n.notice.data?.notice===fact.id);
  },'The quota warning did not arrive in the notice history');
  assert.equal(notice.notice.category,'usage');
  assert.equal(notice.notice.title,'Almost Out');
});
test('a hidden limit learns a new baseline without a stale warning when shown again',async t=>{
  if(boxBinary().typescript)return t.skip('The Rust fixture uses accepted Codex turn samples.');
  const layout=placed=>({layout:{placed,providerOrder:[],metricOrder:{},pinned:[],expanded:[],expandOnEnable:[],seeded:[]}});
  for(const [hide,show] of [[{providers:{known:['codex'],enabled:[]}},{providers:{known:['codex'],enabled:['codex']}}],[layout([]),layout(['codex.session'])]]){
    const {sample,setting,codex}=await noticeFixture(t);
    await sample(89);
    await setting(hide);
    assert.equal((await sample(91)).notices.length,0);
    await setting(show);
    assert.equal((await sample(92)).notices.length,0);
    await codex.stop();
  }
});
test('a retained usage alert catches up after restart with the maker time, not the restart time',async t=>{
  if(boxBinary().typescript)return t.skip('The fixture exercises the Rust retained facts and shared notice history.');
  const fs=await import('node:fs');
  const {sample,codex}=await noticeFixture(t);
  await sample(89);
  const journal=path.join(codex.home,'run','notices','told.json');
  fs.rmSync(journal,{force:true});
  fs.mkdirSync(journal,{recursive:true}); // The fixture makes only this derived journal unwritable.
  const data=await sample(91);
  const fact=data.notices.find(n=>n.milestone==='underTenPercent');assert.ok(fact);
  fs.rmSync(journal,{recursive:true});
  await sample(92);
  const retry=await until(async()=>{
    const result=await request(codex.listener,'GET','kuvo/notices');
    return result.json.notices.find(n=>n.notice.data?.notice===fact.id);
  },'A failed in-app receipt was not retried on the next observation');
  assert.equal(retry.at,fact.at);
  await codex.stop();
  const cache=path.join(codex.home,'agents','usage','accepted-limits.json');
  const kept=JSON.parse(fs.readFileSync(cache,'utf8'));
  const at=Date.now()-120000;
  for(const entry of Object.values(kept.entries))for(const notice of entry.notices??[])if(notice.id===fact.id){
    notice.at=at;
    entry.notices.push({...notice,id:'fixture-old-alert',at:Date.now()-660000},{...notice,id:'fixture-expired-alert',resetsAt:Date.now()-1});
    break;
  }
  write(cache,JSON.stringify(kept));
  // A crash or failed journal write may leave the accepted fact without its
  // in-app receipt. Restart must catch up that still-current fact once.
  fs.rmSync(journal,{force:true});
  let listener=await codex.start();
  const read=async()=>{const result=await request(listener,'GET','kuvo/notices');assert.equal(result.status,200,result.text);return result.json.notices.filter(n=>n.notice.data?.notice===fact.id);};
  const caught=await until(async()=>{const notices=await read();return notices.length&&notices;},'A retained usage alert did not reach the notice history after restart');
  assert.equal(caught.length,1);assert.equal(caught[0].at,at);assert.equal(caught[0].notice.data.at,String(at));
  const all=(await request(listener,'GET','kuvo/notices')).json.notices;
  assert.equal(all.some(n=>['fixture-old-alert','fixture-expired-alert'].includes(n.notice.data?.notice)),false);
  await codex.stop();listener=await codex.start();assert.equal((await read()).length,1);
});
test('the Helper menu and warning facts use the same accepted reading (Rust correction)',async t=>{
  if(boxBinary().typescript)return t.skip('The common reading correction is Rust-only.');
  const {sample,read}=await noticeFixture(t);
  await sample(89);
  const crossed=await sample(91);
  const menu=await read('kuvo/box/at-work');
  const ring=menu.rings.find(r=>r.service==='codex').short;
  assert.equal(ring.used,crossed.rows.find(r=>r.service==='codex').rings.short.used);
  assert.equal(Math.round((1-ring.used)*100),Math.round(crossed.notices.find(n=>n.milestone==='underTenPercent').remaining*100));
  assert.equal(ring.level,'over');
});
test('a remote usage warning is visible in the existing merged notice feed without a second local copy',{timeout:120000},async t=>{
  if(boxBinary().typescript)return t.skip('The reference scratch resident disables provider reads.');
  const http=await import('node:http');
  const {openLine}=await import('./adapter.mjs');
  const now=Date.now(),reset=Math.floor(now/1000)+7200;
  let used=89;
  const maker=http.createServer((req,res)=>{
    assert.equal(req.headers.authorization,'Bearer fixture-notice-holder');
    res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({plan_type:'plus',rate_limit:{primary_window:{used_percent:used,limit_window_seconds:18000,reset_at:reset}}}));
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer,leases}=await bench(t);
  const env=folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'});
  const studio=await computer('NoticeStudio',undefined,env),lab=await computer('NoticeLab',undefined,env);
  studio.engine.credentials=[{integrationID:'openai',active:true,value:{type:'oauth',access:'fixture-notice-holder',expires:now+3600000,metadata:{accountID:'fixture-notice-owner'}}}];
  const main=studio.connect(await studio.start().ready()),base='kuvo/agents/api/kuvo/limits';
  assert.equal((await main('PUT',route,{notifications:{underTenPercent:true,healthyToClose:false,closeToRunningOut:false}})).status,200);
  const refresh=async value=>{
    assert.equal((await main('POST',base+'/refresh',{})).status,200);
    return until(async()=>{const data=(await main('GET',base)).json.data;return !data.refreshPending&&data.rows.find(r=>r.provider==='openai')?.rings.short?.used===value&&data;},'Notice maker reading did not settle');
  };
  await refresh(.89);
  const setup=await main('GET','kuvo/setup');
  const invite=await main('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});assert.equal(invite.status,201,invite.text);
  await until(()=>leases.leaseFor(setup.json.setup),'NoticeStudio did not claim its relay lease');
  const joined=await lab.join(invite.json.code);assert.equal(joined.code,0,joined.log);
  const listener=await lab.start().ready(),other=lab.connect(listener);
  await until(async()=>{const data=(await other('GET',base)).json.data;return data.rows.find(r=>r.provider==='openai')?.rings.short?.used===.89;},'Lab did not learn the source account',30000,100);
  const line=await openLine(listener,'','kuvo/setup/notices/events',{},30000);
  try{
    assert.equal(line.status,200);
    used=91;const crossed=await refresh(.91);const fact=crossed.notices.find(n=>n.milestone==='underTenPercent');assert.ok(fact);
    let found;
    for await(const frame of line.frames()){
      const matches=(frame.notices??[]).filter(n=>n.notice?.data?.notice===fact.id);
      if(matches.length){found=matches;break;}
    }
    assert.equal(found?.length,1);assert.equal(found[0].computer.name,'NoticeStudio');assert.equal(found[0].at,fact.at);
    assert.ok(!(await other('GET','kuvo/notices')).json.notices.some(n=>n.notice.data?.notice===fact.id));
  }finally{line.close();}
});

test('usage import is once-only and older clients preserve settings they do not know',async t=>{
  const {computer}=await bench(t);
  const box=await computer('Import',undefined,isolated);
  const resident=box.start();
  const call=box.connect(await resident.ready());
  const first=await call('POST',route+'/import',{totalSpend:{show:true,futureField:'kept'},codexFallback:'not-a-real-model'});
  assert.equal(first.status,200,first.text);
  assert.deepEqual(first.json.data.took,['totalSpend']);
  assert.equal(first.json.data.imported,true);
  const again=await call('POST',route+'/import',{totalSpend:{show:false}});
  assert.equal(again.status,200,again.text);
  assert.deepEqual(again.json.data.took,[]);
  const edited=await call('PUT',route,{totalSpend:{show:false}});
  assert.equal(edited.status,200,edited.text);
  assert.deepEqual(edited.json.data.settings.totalSpend,{show:false,futureField:'kept'});
  const invalid=await call('PUT',route,{notifications:{underTenPercent:true}});
  assert.equal(invalid.status,400,invalid.text);
});

test('manual refresh reads Codex on demand, not on an idle startup (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('The idle-cost change is Rust-only.');
  const codex=await scriptedCodex();
  t.after(()=>codex.close());
  const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:'fixture',chatgpt_user_id:'fixture-user'}})).toString('base64url'),'fixture'].join('.');
  write(path.join(codex.root,'.codex','auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:'fixture'}}));
  const reset=Math.floor(Date.now()/1000)+7200;
  write(path.join(codex.root,'codex-limits.json'),JSON.stringify({rateLimits:{primary:{usedPercent:27,windowDurationMins:300,resetsAt:reset}}}));
  const listener=await codex.start();
  assert.equal(codex.sent('account/rateLimits/read').length,0);
  const updated=await request(listener,'POST','kuvo/agents/api/kuvo/limits/refresh',{});
  assert.equal(updated.status,200,updated.text);
  const data=await until(async ()=>{
    const reply=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
    return reply.json.data.rows.find(row=>row.provider==='codex-cli')?.rings.short?.used===0.27;
  },'Refresh did not publish the maker’s current reading');
  assert.ok(data);
  assert.equal(codex.sent('account/rateLimits/read').length,1);
});

test('Insights catches up before first open and folds changed transcripts without double-counting (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('The file-driven index is a Rust change.');
  const {computer}=await bench(t);
  const box=await computer('Insights',undefined,isolated);
  const now=Date.now();
  const id='ses_usage_insight';
  const model={providerID:'claude-code',id:'claude-opus-4-6'};
  write(path.join(box.home,'agents','sessions',id,'session.json'),JSON.stringify({id,title:'A fixture',model,time:{created:now-1000,updated:now},location:{directory:box.home}}));
  write(path.join(box.home,'agents','sessions',id,'messages.json'),JSON.stringify([{id:'step_fixture',type:'assistant',time:{created:now-1000,completed:now},model,tokens:{input:100,output:50,cache:{read:500,write:2}},content:[]}]));
  const listener=await box.start().ready();
  const call=box.connect(listener);
  const db=path.join(box.home,'agents','usage','insights.sqlite');
  const {existsSync}=await import('node:fs');
  const {DatabaseSync}=await import('node:sqlite');
  const counted=()=>{
    if(!existsSync(db))return 0;
    let index;
    try {index=new DatabaseSync(db,{readOnly:true});return Number(index.prepare('SELECT COUNT(*) AS n FROM event WHERE human=0').get().n);}
    catch{return 0;}
    finally{index?.close();}
  };
  await until(()=>counted()===1,'The idle startup catch-up did not index the fixture before opening Insights');
  const first=await call('GET','kuvo/agents/api/kuvo/insights?period=7&zone=UTC');
  assert.equal(first.status,200,first.text);
  assert.equal(first.json.data.totals.calls,1);
  assert.equal(first.json.data.totals.processed,652);
  assert.equal(first.json.data.totals.tokens,152);
  assert.equal(existsSync(db),true);
  const second=await call('GET','kuvo/agents/api/kuvo/insights?period=7&zone=UTC');
  assert.equal(second.status,200,second.text);
  assert.deepEqual(second.json.data.totals,first.json.data.totals);
  const messages=path.join(box.home,'agents','sessions',id,'messages.json');
  const {readFileSync}=await import('node:fs');
  const original=JSON.parse(readFileSync(messages,'utf8'));
  write(messages,JSON.stringify([...original,{...original[0],id:'step_fixture_2'}]));
  await until(()=>counted()===2,'A changed transcript was not folded before the next Insights query');
  const updated=await call('GET','kuvo/agents/api/kuvo/insights?period=7&zone=UTC');
  assert.equal(updated.json.data.totals.calls,2);
  assert.equal(updated.json.data.totals.processed,1304);
});

test('a subscribed rail reads Codex logs without starting an app-server (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('The demand-driven reader is Rust-only.');
  const codex=await scriptedCodex();
  t.after(()=>codex.close());
  const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:'fixture',chatgpt_user_id:'fixture-user'}})).toString('base64url'),'fixture'].join('.');
  write(path.join(codex.root,'.codex','auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:'fixture'}}));
  const reset=Math.floor(Date.now()/1000)+7200;
  const log=path.join(codex.root,'.codex','sessions','rate-limits.jsonl');
  const listener=await codex.start();
  const {openLine}=await import('./adapter.mjs');
  const line=await openLine(listener,'','kuvo/agents/api/kuvo/limits/events');
  try {
    assert.equal(line.status,200);
    write(log,JSON.stringify({timestamp:new Date().toISOString(),type:'event_msg',payload:{type:'token_count',rate_limits:{primary:{used_percent:31,window_minutes:300,resets_at:reset}}}})+'\n');
    for await(const frame of line.frames()){
      if(frame.changed){
        const answer=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
        if(answer.json.data.rows.find(r=>r.provider==='codex-cli')?.rings.short?.used===0.31)break;
      }
    }
    assert.equal(codex.sent('account/rateLimits/read').length,0);
    assert.equal(codex.sent('initialize').length,0);
    const fs=await import('node:fs');
    fs.unlinkSync(path.join(codex.root,'.codex','auth.json'));
    const signedOut=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
    assert.ok(!signedOut.json.data.accounts.some(a=>a.provider==='codex-cli'));
  } finally {line.close();}
});

test('a portable transcript contributes its tokens and cache reads to Insights on either box',async t=>{
  const {computer}=await bench(t);
  const box=await computer('Portable',undefined,isolated);
  const now=Date.now()-1000;
  const id='ses_usage_portable';
  const model={providerID:'claude-code',id:'claude-opus-4-6'};
  write(path.join(box.home,'agents','sessions',id,'session.json'),JSON.stringify({id,title:'Usage fixture',model,agent:'build',projectID:'fixture',time:{created:now-1000,updated:now},location:{directory:box.home}}));
  write(path.join(box.home,'agents','sessions',id,'agent.json'),JSON.stringify({threads:{}}));
  write(path.join(box.home,'agents','sessions',id,'messages.json'),JSON.stringify([{id:'step_fixture',type:'assistant',time:{created:now-1000,completed:now},model,agent:'build',tokens:{input:100,output:50,cache:{read:500,write:2}},content:[]}]));
  const listener=await box.start().ready();
  const call=box.connect(listener);
  const answer=await call('GET','kuvo/agents/api/kuvo/insights/summary?period=7&zone=UTC&refresh=1');
  assert.equal(answer.status,200,answer.text);
  assert.equal(answer.json.data.totals.tokens,652);
  assert.equal(answer.json.data.totals.cacheRate,500/602);
});

test('the empty limits answer states every service and refresh state in the contract order',async t=>{
  const {computer}=await bench(t);
  const box=await computer('Limits',undefined,isolated);
  const listener=await box.start().ready();
  const answer=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
  assert.equal(answer.status,200,answer.text);
  const data=answer.json.data;
  assert.deepEqual(data.services,['claude','codex','cursor','grok','antigravity','copilot','openrouter','zai','opencode']);
  assert.equal(data.refreshFailed,false);
  assert.equal(data.refreshPending,false);
  assert.deepEqual(data.accounts,[]);
  assert.deepEqual(data.rows,[]);
});

test('an unreadable accepted-limit cache cannot prevent the resident from opening or be overwritten (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('The acceptance cache is a Rust format.');
  const {computer}=await bench(t);
  const box=await computer('FutureLimits',undefined,isolated);
  const file=path.join(box.home,'agents','usage','accepted-limits.json');
  const kept=JSON.stringify({version:999,entries:{}});
  write(file,kept);
  const call=box.connect(await box.start().ready());
  const answer=await call('GET','kuvo/agents/api/kuvo/limits');
  assert.equal(answer.status,200,answer.text);
  assert.equal(answer.json.data.refreshFailed,true);
  const {readFileSync}=await import('node:fs');
  assert.equal(readFileSync(file,'utf8'),kept);
});

test('engine credential plans are read with their own sign-ins and display bounded counts and credits (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('This conformance fixture injects the Rust reader’s provider endpoints.');
  const http=await import('node:http');
  const crypto=await import('node:crypto');
  const answered=[];
  const now=Date.now();
  const reset=Math.floor(now/1000)+7200;
  const replies={
    '/chatgpt':{plan_type:'plus',rate_limit:{primary_window:{used_percent:30,limit_window_seconds:18000,reset_at:reset}},credits:{has_credits:true,balance:'821.9'},rate_limit_reset_credits:{available_count:0}},
    '/grok':{config:{currentPeriod:{start:new Date(now-3*86400000).toISOString(),end:new Date(now+4*86400000).toISOString()},creditUsagePercent:64}},
    '/copilot':{login:'Ada-L',copilot_plan:'individual_pro',quota_reset_date:new Date(now+86400000).toISOString(),quota_snapshots:{premium_interactions:{entitlement:300,remaining:120,percent_remaining:40}}},
    '/openrouterKey':{data:{limit:null,is_free_tier:false}},
    '/openrouterCredits':{data:{total_credits:20,total_usage:7.5}},
    '/zai':{data:{limits:[{type:'TOKENS_LIMIT',unit:3,number:5,percentage:12,nextResetTime:reset*1000},{type:'TIME_LIMIT',currentValue:40,usage:100}]}},
    '/opencodeGo':{usage:{rolling:{percent:9,resetsAt:new Date(reset*1000).toISOString()},weekly:{percent:21,resetsAt:new Date(now+4*86400000).toISOString()}}}
  };
  let rejectRouter=false;
  const provider=http.createServer((req,res)=>{
    answered.push({url:req.url,auth:req.headers.authorization,account:req.headers['chatgpt-account-id']});
    if(rejectRouter&&req.url==='/openrouterKey'){res.writeHead(401,{'Content-Type':'application/json'});res.end('{}');return;}
    const reply=replies[req.url];
    res.writeHead(reply?200:404,{'Content-Type':'application/json'});res.end(JSON.stringify(reply??{}));
  });
  await new Promise(resolve=>provider.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{provider.close(resolve);provider.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('EngineLimits',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+provider.address().port+'/'}));
  const sign=(integrationID,value,active=true)=>({integrationID,active,value});
  box.engine.credentials=[
    sign('openai',{type:'oauth',access:'fixture-chatgpt',expires:now+3600000,metadata:{accountID:'fixture-account'}}),
    sign('xai',{type:'oauth',access:['e30',Buffer.from(JSON.stringify({sub:'fixture-xai-user'})).toString('base64url'),'fixture'].join('.'),expires:now+3600000}),
    sign('github-copilot',{type:'oauth',refresh:'fixture-github',access:'short-lived',expires:now-1}),
    sign('openrouter',{type:'api',key:'fixture-router'}),
    sign('zai-coding-plan',{type:'api',key:'fixture-zai'}),
    sign('opencode-go',{type:'api',key:'fixture-go'}),
    sign('anthropic',{type:'api',key:'unused'},false),
  ];
  const call=box.connect(await box.start().ready());
  assert.equal(answered.length,0);
  const reply=await call('POST','kuvo/agents/api/kuvo/limits/refresh',{});
  assert.equal(reply.status,200,reply.text);
  const data=await until(async()=>{
    const response=await call('GET','kuvo/agents/api/kuvo/limits');
    return !response.json.data.refreshPending&&response.json.data;
  },'Engine plan read did not settle');
  const accounts=Object.fromEntries(data.accounts.map(row=>[row.provider,row]));
  assert.equal(accounts.openai.plan,'Plus');
  assert.equal(accounts.openai.windows[0].used,0.3);
  assert.deepEqual(accounts.openai.extras,[{kind:'left',id:'credits',name:'Extra Usage',amount:821,unit:'credits'},{kind:'resets',id:'resets',name:'Rate Limit Resets',count:0,usable:false}]);
  assert.equal(accounts.xai.windows[0].used,0.64);
  assert.deepEqual(accounts['github-copilot'].windows[0].amount,{used:180,limit:300,unit:'requests'});
  assert.deepEqual(accounts.openrouter.balance,{left:12.5,limit:20,unit:'usd'});
  assert.equal(accounts['zai-coding-plan'].windows.find(w=>w.id==='web_search').used,0.4);
  assert.equal(accounts['opencode-go'].windows.find(w=>w.id==='rolling').used,0.09);
  assert.equal(answered.find(r=>r.url==='/copilot').auth,'token fixture-github');
  assert.deepEqual(answered.find(r=>r.url==='/chatgpt'),{url:'/chatgpt',auth:'Bearer fixture-chatgpt',account:'fixture-account'});
  assert.equal(data.refreshFailed,false);
  const identity=raw=>crypto.createHash('sha256').update('kuvo-code plan limits account v1\n'+raw).digest('hex').slice(0,16);
  assert.equal(accounts.openrouter.identity,identity('key:fixture-router'));
  assert.equal(accounts['github-copilot'].identity,identity('github:ada-l'));
  assert.equal(accounts.xai.identity,identity('xai:fixture-xai-user'));
  for(const secret of ['Ada-L','ada-l','fixture-router','fixture-github','fixture-xai-user'])assert.ok(!JSON.stringify(data).includes(secret));
  const base='kuvo/agents/api/kuvo/limits';
  const refresh=async()=>{assert.equal((await call('POST',base+'/refresh',{})).status,200);return until(async()=>{const response=await call('GET',base);return !response.json.data.refreshPending&&response.json.data;},'Engine fixture refresh did not settle');};
  // GitHub logins are case-insensitive, including when two sign-ins name the
  // same account. A changed credential must not inherit another login's ring.
  replies['/copilot'].login='ada-l';box.engine.credentials=box.engine.credentials.map(c=>c.integrationID==='github-copilot'?sign(c.integrationID,{...c.value,refresh:'fixture-github-other'}):c);
  const same=(await refresh()).accounts.find(a=>a.provider==='github-copilot');assert.equal(same.identity,accounts['github-copilot'].identity);
  replies['/copilot'].login='Bob';replies['/copilot'].quota_snapshots.premium_interactions={entitlement:300,remaining:290,percent_remaining:290/300*100};
  box.engine.credentials=box.engine.credentials.map(c=>c.integrationID==='github-copilot'?sign(c.integrationID,{...c.value,refresh:'fixture-github-bob'}):c);
  const switched=(await refresh()).accounts.find(a=>a.provider==='github-copilot');
  assert.equal(switched.identity,identity('github:bob'));assert.deepEqual(switched.windows[0].amount,{used:10,limit:300,unit:'requests'});
  rejectRouter=true;const denied=(await refresh()).accounts.find(a=>a.provider==='openrouter');assert.equal(denied.problem.kind,'failed');assert.deepEqual(denied.balance,accounts.openrouter.balance);
  const count=answered.filter(r=>r.url==='/openrouterKey').length;
  rejectRouter=false;const restored=(await refresh()).accounts.find(a=>a.provider==='openrouter');
  assert.equal(restored.problem,undefined);assert.equal(restored.identity,accounts.openrouter.identity);assert.equal(answered.filter(r=>r.url==='/openrouterKey').length,count+1);
});

test('Claude Code answers its plan via its own demand-started control process (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('This fixture supplies the Rust native usage-control reader.');
  const {scriptedClaude}=await import('./claude.mjs');
  const claude=await scriptedClaude();
  t.after(()=>claude.close());
  write(path.join(claude.env.CLAUDE_CONFIG_DIR,'.claude.json'),JSON.stringify({oauthAccount:{accountUuid:'usage-fixture',organizationUuid:'fixture-org'}}));
  write(path.join(claude.env.CLAUDE_CONFIG_DIR,'.credentials.json'),JSON.stringify({claudeAiOauth:{accessToken:'fixture-plan',expiresAt:Date.now()+3600000}}));
  const reset=new Date(Date.now()+7200000).toISOString();
  write(path.join(claude.env.HOME,'claude-usage.json'),JSON.stringify({subscription_type:'max',rate_limits_available:true,rate_limits:{five_hour:{utilization:42,resets_at:reset},seven_day:{utilization:17,resets_at:reset},extra_usage:{is_enabled:true,used_credits:1234,monthly_limit:5000}}}));
  const listener=await claude.start();
  assert.equal(claude.heard().filter(row=>row.message?.request?.subtype==='get_usage').length,0);
  const answer=await request(listener,'POST','kuvo/agents/api/kuvo/limits/refresh',{});
  assert.equal(answer.status,200,answer.text);
  const account=await until(async()=>{
    const response=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
    return response.json.data.accounts.find(row=>row.provider==='claude-code'&&row.windows.length);
  },'Claude did not report the plan’s usage');
  assert.equal(account.plan,'Max');
  assert.equal(account.windows.find(w=>w.id==='five_hour').used,0.42);
  assert.deepEqual(account.extras.find(e=>e.id==='extra_usage'),{kind:'capped',id:'extra_usage',name:'Extra Usage',amount:12.34,limit:50,unit:'usd'});
  // The account identity file alone is not an OAuth sign-in.
  const fs=await import('node:fs');
  fs.unlinkSync(path.join(claude.env.CLAUDE_CONFIG_DIR,'.credentials.json'));
  const signedOut=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
  assert.ok(!signedOut.json.data.accounts.some(a=>a.provider==='claude-code'));
});

test('a Codex reset is offered without spending, journaled before the maker, and replayed once after restart (Rust reader)', async t=>{
  if(boxBinary().typescript)return t.skip('This fixture supplies the Rust reset journal and native Codex account reader.');
  const codex=await scriptedCodex();
  t.after(()=>codex.close());
  const makeAuth=id=>['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:id,chatgpt_user_id:'fixture-user'}})).toString('base64url'),'fixture'].join('.');
  const auth=path.join(codex.root,'.codex','auth.json');
  write(auth,JSON.stringify({tokens:{access_token:makeAuth('fixture'),account_id:'fixture'}}));
  const reset=Math.floor(Date.now()/1000)+7200;
  write(path.join(codex.root,'codex-limits.json'),JSON.stringify({accountId:'fixture',rateLimits:{planType:'plus',primary:{usedPercent:100,windowDurationMins:300,resetsAt:reset}},rateLimitResetCredits:{availableCount:2,credits:[{id:'late',status:'available',expiresAt:reset+7200},{id:'soon',status:'available',expiresAt:reset+3600}]}}));
  let listener=await codex.start();
  const base='kuvo/agents/api/kuvo/limits';
  const refresh=async()=>{
    const reply=await request(listener,'POST',base+'/refresh',{});
    assert.equal(reply.status,200,reply.text);
    return until(async()=>{
      const r=await request(listener,'GET',base);
      return !r.json.data.refreshPending&&r.json.data.rows.find(row=>row.provider==='codex-cli'&&row.extras?.some(e=>e.kind==='resets'));
    },'The Codex reset inventory did not arrive');
  };
  const read=await refresh();
  const row=read.id;
  assert.deepEqual(read.card.resets.words,[{text:'2 resets left',tone:'muted'},{text:' · ',tone:'faint'},{text:'next expires ',at:(reset+3600)*1000,as:'date',tone:'muted'}]);
  assert.equal(read.card.resets.action,'Use a reset');
  assert.equal(read.card.resets.confirm.message,'Codex’s usage limits start over now.');
  const post=body=>request(listener,'POST',base+'/reset',body);
  const offer=await post({row});
  assert.equal(offer.status,200,offer.text);
  assert.equal(offer.json.data.count,2);
  assert.equal(offer.json.data.always,false);
  assert.equal(codex.sent('account/rateLimitResetCredit/consume').length,0);
  // The maker sees the spend but cannot say it went through. The resident is restarted.
  write(path.join(codex.root,'codex-reset-answer.json'),JSON.stringify({outcome:'unknown'}));
  const confirm=offer.json.data.confirm;
  const unknown=await post({row,confirm});
  assert.equal(unknown.status,200,unknown.text);
  assert.equal(unknown.json.data.outcome,'unknown');
  const first=codex.sent('account/rateLimitResetCredit/consume');
  assert.equal(first.length,1);
  assert.equal(first[0].params.creditId,'soon');
  await codex.stop();
  listener=await codex.start();
  const again=await post({row,confirm});
  assert.equal(again.status,200,again.text);
  assert.equal(again.json.data.outcome,'reset');
  const twice=codex.sent('account/rateLimitResetCredit/consume');
  assert.equal(twice.length,2);
  assert.deepEqual(twice[1].params,twice[0].params);
  const done=await post({row,confirm});
  assert.equal(done.json.data.outcome,'reset');
  assert.equal(codex.sent('account/rateLimitResetCredit/consume').length,2);
  // A second offer for the old account must not spend from a newly signed in one.
  const old=await post({row});
  assert.equal(old.status,200,old.text);
  write(auth,JSON.stringify({tokens:{access_token:makeAuth('other'),account_id:'other'}}));
  const refused=await post({row,confirm:old.json.data.confirm});
  assert.notEqual(refused.json?.data?.outcome,'reset');
  assert.equal(codex.sent('account/rateLimitResetCredit/consume').length,2);
});

test('an agent has to ask before spending a reset, and Always Allow is account-specific (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('This fixture supplies the Rust reset journal and native Codex reader.');
  const codex=await scriptedCodex();
  t.after(()=>codex.close());
  const {standInEngine,sessionOf}=await import('./engine.mjs');
  const engine=await standInEngine(path.dirname(codex.home),codex.env);
  t.after(()=>engine.close());
  write(path.join(codex.home,'run','engine.json'),JSON.stringify(engine.record));
  const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:'fixture',chatgpt_user_id:'fixture-user'}})).toString('base64url'),'fixture'].join('.');
  write(path.join(codex.root,'.codex','auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:'fixture'}}));
  const reset=Math.floor(Date.now()/1000)+7200;
  write(path.join(codex.root,'codex-limits.json'),JSON.stringify({accountId:'fixture',rateLimits:{planType:'plus',primary:{usedPercent:100,windowDurationMins:300,resetsAt:reset}},rateLimitResetCredits:{availableCount:2,credits:[{id:'late',status:'available',expiresAt:reset+7200},{id:'soon',status:'available',expiresAt:reset+3600}]}}));
  const session='ses_resetask';
  engine.sessions.push(sessionOf({id:session,title:'Reset fixture',directory:codex.root}));
  const listener=await codex.start();
  const base='kuvo/agents/api/kuvo/limits';
  const refresh=await request(listener,'POST',base+'/refresh',{});
  assert.equal(refresh.status,200,refresh.text);
  const row=await until(async()=>{
    const reply=await request(listener,'GET',base);
    return !reply.json.data.refreshPending&&reply.json.data.rows.find(r=>r.provider==='codex-cli'&&r.extras?.some(e=>e.kind==='resets'));
  },'The Codex reset inventory did not arrive');
  const tool=(name,args={})=>request(listener,'POST','kuvo/agent-tools',{name,arguments:args,session},undefined,20000);
  const limits=await tool('plan_limits');
  assert.match(limits.json.text,/rate-limit resets: 2 available/);
  const ask=async(answer)=>{
    const call=tool('use_limit_reset',{account:row.id});
    const card=await until(async()=>{
      const reply=await request(listener,'GET','kuvo/work/asks');
      const list=reply.json.asks??reply.json.data??[];
      return list.find(a=>a.action==='resetLimits'&&a.reset?.row===row.id);
    },'The reset must ask the user');
    assert.equal(card.reset.count,2);
    assert.equal(card.folder,null);
    assert.match(card.words.title,/asks to use a reset/);
    assert.equal(codex.sent('account/rateLimitResetCredit/consume').length,0);
    const answered=await request(listener,'POST','kuvo/work/asks/'+card.id,{answer},undefined,20000);
    assert.equal(answered.status,200,answered.text);
    return call;
  };
  const denied=await ask('deny');
  assert.equal(denied.json.isError,true);
  assert.equal(codex.sent('account/rateLimitResetCredit/consume').length,0);
  const allowed=await ask('always');
  assert.equal(allowed.json.isError,false,allowed.text);
  assert.equal(codex.sent('account/rateLimitResetCredit/consume').length,1);
  const snapshot=await request(listener,'GET',base);
  assert.equal(snapshot.json.data.rows.find(r=>r.id===row.id).extras.find(e=>e.kind==='resets').agents,true);
  const off=await request(listener,'PUT',base+'/reset/always',{row:row.id,always:false});
  assert.equal(off.status,200,off.text);
  const offered=await request(listener,'POST',base+'/reset',{row:row.id});
  assert.equal(offered.json.data.always,false);
  const unasked=await request(listener,'POST',base+'/reset',{row:row.id,confirm:offered.json.data.confirm,unasked:true});
  assert.equal(unasked.json.data.outcome,'failed');
  assert.equal(codex.sent('account/rateLimitResetCredit/consume').length,1);
});

test('a connected Helper menu displays fresh log rings without launching Codex (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('This case supplies the Rust native demand reader.');
  const codex=await scriptedCodex();
  t.after(()=>codex.close());
  const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:'fixture',chatgpt_user_id:'fixture-user'}})).toString('base64url'),'fixture'].join('.');
  write(path.join(codex.root,'.codex','auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:'fixture'}}));
  const reset=Math.floor(Date.now()/1000)+7200;
  const log=path.join(codex.root,'.codex','sessions','rate-limits.jsonl');
  const listener=await codex.start();
  assert.equal(codex.sent('account/rateLimits/read').length,0);
  const {openLine}=await import('./adapter.mjs');
  const line=await openLine(listener,'','kuvo/box/at-work/events');
  try {
    assert.equal(line.status,200);
    write(log,JSON.stringify({timestamp:new Date().toISOString(),type:'event_msg',payload:{type:'token_count',rate_limits:{primary:{used_percent:43,window_minutes:300,resets_at:reset}}}})+'\n');
    for await(const frame of line.frames()) {
      if(frame.rings?.find(r=>r.service==='codex')?.short?.used===0.43)break;
    }
    const now=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
    assert.equal(now.json.data.rows.find(r=>r.provider==='codex-cli').rings.short.used,0.43);
    assert.equal(codex.sent('account/rateLimits/read').length,0);
    assert.equal(codex.sent('initialize').length,0);
  } finally {line.close();}
});

test('reset routes refuse missing or unknown accounts without creating a spend (both boxes)',async t=>{
  const {computer}=await bench(t);
  const box=await computer('UnknownReset',undefined,isolated);
  const call=box.connect(await box.start().ready());
  const route='kuvo/agents/api/kuvo/limits/reset';
  const missing=await call('POST',route,{});
  assert.equal(missing.status,400,missing.text);
  assert.equal(missing.json.message,'Say which account (row).');
  const unknown=await call('POST',route,{row:'codex#unknown'});
  assert.equal(unknown.status,404,unknown.text);
  assert.equal(unknown.json.message,'No account of yours by that name.');
  const invalid=await call('PUT',route+'/always',{row:'codex#unknown'});
  assert.equal(invalid.status,400,invalid.text);
  assert.equal(invalid.json.message,'Say which account (row or provider), and always.');
});

test('standalone Grok, Cursor and Antigravity read their own fixture accounts (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('The TypeScript scratch bridge disables live provider reads.');
  const http=await import('node:http');
  const fs=await import('node:fs');
  const now=Date.now();
  const end=now+7*86400000;
  const access=['e30',Buffer.from(JSON.stringify({sub:'workos|cursor-fixture',exp:Math.floor(now/1000)+3600})).toString('base64url'),'fixture'].join('.');
  const jwt=['e30',Buffer.from(JSON.stringify({sub:'google-fixture'})).toString('base64url'),'fixture'].join('.');
  const requests=[];
  const bucket=(bucketId,remainingFraction,resetAt=end)=>({bucketId,remainingFraction,resetTime:new Date(resetAt).toISOString()});
  const replies={
    '/cursor/exchange':{accessToken:access},
    '/cursor/GetCurrentPeriodUsage':{enabled:true,billingCycleStart:now-10*86400000,billingCycleEnd:now+20*86400000,planUsage:{limit:10000,totalSpend:2000,totalPercentUsed:20,autoPercentUsed:12,apiPercentUsed:37},spendLimitUsage:{individualLimit:5000,individualUsed:1234}},
    '/cursor/GetPlanInfo':{planInfo:{planName:'pro'}},
    '/cursor/GetCreditGrantsBalance':{hasCreditGrants:true,totalCents:2000,usedCents:500},
    '/cursor/auth/stripe':{customerBalance:-200},
    '/cursor/GetSandUsageStatus':{usagePercent:8,currentPeriodStart:new Date(now).toISOString(),nextResetTimestampUtc:new Date(end).toISOString()},
    '/antigravity/main/v1internal:retrieveUserQuotaSummary':{groups:[{displayName:'Gemini',buckets:[bucket('gemini-5h',0.2,now+7200000),bucket('gemini-weekly',0.7)]}]},
    '/antigravity/daily/v1internal:retrieveUserQuotaSummary':{groups:[{displayName:'Claude and GPT',buckets:[bucket('3p-5h',0.4,now+7200000),bucket('3p-weekly',0.9)]}]},
    '/antigravity/main/v1internal:loadCodeAssist':{paidTier:{id:'g1-pro-tier'}},
  };
  const provider=http.createServer((req,res)=>{
    requests.push({url:req.url,auth:req.headers.authorization,cookie:req.headers.cookie});
    const body=replies[req.url];
    res.writeHead(body?200:404,{'Content-Type':'application/json'});res.end(JSON.stringify(body??{}));
  });
  await new Promise(resolve=>provider.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{provider.close(resolve);provider.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('Standalone',undefined,folder=>{
    const env=isolated(folder);
    const bin=path.join(folder,'bin');fs.mkdirSync(bin,{recursive:true});
    const program=String.raw`
const fs=require('node:fs'),path=require('node:path'),readline=require('node:readline');
const home=process.env.GROK_HOME;
let authed=false;
readline.createInterface({input:process.stdin}).on('line',line=>{
  const request=JSON.parse(line);
  fs.appendFileSync(path.join(home,'heard.jsonl'),JSON.stringify(request)+'\n');
  const answer=result=>console.log(JSON.stringify({jsonrpc:'2.0',id:request.id,result}));
  if(request.method==='initialize')answer({protocolVersion:1});
  else if(request.method==='authenticate'){authed=request.params.methodId===(process.env.XAI_API_KEY?'xai.api_key':'cached_token');answer({});}
  else if(request.method==='_x.ai/billing'){
    if(!authed)console.log(JSON.stringify({jsonrpc:'2.0',id:request.id,error:{code:-32000,message:'Authentication required'}}));
    else answer(JSON.parse(fs.readFileSync(path.join(home,'billing.json'),'utf8')));
  }
});
`;
    if(process.platform==='win32'){
      fs.writeFileSync(path.join(bin,'grok.cjs'),program);
      fs.writeFileSync(path.join(bin,'grok.cmd'),`@"${process.execPath}" "%~dp0grok.cjs" %*\r\n`);
    }else fs.writeFileSync(path.join(bin,'grok'),`#!${process.execPath}\n${program}`,{mode:0o755});
    const grokJwt=['e30',Buffer.from(JSON.stringify({sub:'grok-fixture'})).toString('base64url'),'fixture'].join('.');
    write(path.join(env.GROK_HOME,'auth.json'),JSON.stringify({fixture:{key:grokJwt}}));
    write(path.join(env.GROK_HOME,'billing.json'),JSON.stringify({config:{currentPeriod:{start:new Date(now).toISOString(),end:new Date(end).toISOString()},creditUsagePercent:61}}));
    return {...env,PATH:[bin,process.platform==='win32'?path.join(process.env.SystemRoot,'System32'):'/usr/bin:/bin'].join(path.delimiter), KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+provider.address().port+'/'};
  });
  write(path.join(box.home,'agents','cursor','auth.json'),JSON.stringify({apiKey:'fixture-cursor'}));
  write(path.join(box.home,'agents','antigravity','profile','antigravity-acp','acp_token.json'),JSON.stringify({access_token:jwt,expires_at:Math.floor(now/1000)+3600}));
  writeGrokHistory(box.env.GROK_HOME,now-60_000);
  let resident=box.start();
  let call=box.connect(await resident.ready());
  assert.equal(requests.length,0);
  const refresh=async()=>{
    const reply=await call('POST','kuvo/agents/api/kuvo/limits/refresh',{});
    assert.equal(reply.status,200,reply.text);
    return until(async()=>{
      const view=await call('GET','kuvo/agents/api/kuvo/limits');
      return !view.json.data.refreshPending&&view.json.data;
    },'Standalone plans did not settle');
  };
  const data=await refresh();
  const accounts=Object.fromEntries(data.accounts.map(a=>[a.provider,a]));
  assert.equal(accounts['grok-cli'].windows[0].used,0.61);
  assert.equal(accounts.cursor.plan,'Pro');
  assert.equal(accounts.cursor.windows.find(w=>w.id==='total').used,0.2);
  assert.deepEqual(accounts.cursor.balance,{left:17,unit:'usd'});
  assert.equal(accounts.cursor.windows.find(w=>w.id==='on_demand').amount.used,12.34);
  assert.equal(accounts.cursor.windows.find(w=>w.id==='grok_bot').used,0.08);
  assert.equal(accounts.antigravity.plan,'Pro');
  assert.equal(accounts.antigravity.windows.length,4);
  assert.equal(accounts.antigravity.windows.find(w=>w.id==='gemini-5h').used,0.8);
  assert.equal(data.refreshFailed,false);
  const heard=fs.readFileSync(path.join(box.env.GROK_HOME,'heard.jsonl'),'utf8').trim().split('\n').map(line=>JSON.parse(line));
  assert.deepEqual(heard.map(m=>m.method),['initialize','_x.ai/billing','authenticate','_x.ai/billing']);
  assert.equal(requests.find(r=>r.url==='/cursor/exchange').auth,'Bearer fixture-cursor');
  // A single spurious reset and empty ring from Google cannot replace the accepted window.
  replies['/antigravity/main/v1internal:retrieveUserQuotaSummary'].groups[0].buckets[0]=bucket('gemini-5h',1,now+10800000);
  const next=await refresh();
  assert.equal(next.accounts.find(a=>a.provider==='antigravity').windows.find(w=>w.id==='gemini-5h').used,0.8);
  // Signing out externally must withdraw the live rows without waiting for a provider fetch.
  fs.unlinkSync(path.join(box.env.GROK_HOME,'auth.json'));
  fs.unlinkSync(path.join(box.home,'agents','cursor','auth.json'));
  fs.unlinkSync(path.join(box.home,'agents','antigravity','profile','antigravity-acp','acp_token.json'));
  await until(()=>{
    const cache=JSON.parse(fs.readFileSync(path.join(box.home,'agents','usage','accepted-limits.json'),'utf8'));
    return !['grok-cli','cursor','antigravity'].some(id=>cache.entries[id]);
  },'Sign-out did not remove the retained limits while no client was reading');
  const signedOut=await call('GET','kuvo/agents/api/kuvo/limits');
  assert.equal(signedOut.status,200,signedOut.text);
  assert.deepEqual(signedOut.json.data.accounts,[]);
  assert.deepEqual(signedOut.json.data.rows,[]);
  const accountRow=async()=>{const reply=await call('GET','kuvo/agents/api/integration?fresh=1');assert.equal(reply.status,200,reply.text);return reply.json.data.find(r=>r.id==='grok-cli');};
  assert.deepEqual((await accountRow()).connections,[],'Accounts agrees the Grok sign-in is gone');
  const history=await call('POST','kuvo/agents/api/kuvo/usage/refresh?zone=UTC',{});
  assert.ok(history.json.data.rows.find(r=>r.provider==='grok-cli')?.days.some(d=>d.tokens>0));
  // The same environment API-key sign-in Accounts uses also counts as a real
  // credential. The CLI stand-in answers only for that auth method, never a real key.
  await resident.stop();
  resident=box.start(undefined,{XAI_API_KEY:'fixture-key'});
  call=box.connect(await resident.ready());
  const withKey=await refresh();
  assert.equal(withKey.accounts.find(a=>a.provider==='grok-cli')?.windows[0]?.used,0.61);
  const keyCalls=fs.readFileSync(path.join(box.env.GROK_HOME,'heard.jsonl'),'utf8').trim().split('\n').map(JSON.parse);
  assert.equal(keyCalls.filter(m=>m.method==='authenticate').at(-1)?.params.methodId,'xai.api_key');
  assert.ok((await accountRow()).connections.some(c=>c.status.status==='connected'),'Accounts recognises the same environment key');
  const bin=box.env.PATH.split(path.delimiter)[0];fs.unlinkSync(path.join(bin,process.platform==='win32'?'grok.cmd':'grok'));
  const missing=await call('GET','kuvo/agents/api/kuvo/limits');assert.ok(!missing.json.data.rows.some(r=>r.provider==='grok-cli'));
  assert.deepEqual((await accountRow()).connections,[],'Accounts and live limits also agree when the CLI disappears');
});

function writeGrokHistory(folder,at){
  const update={sessionUpdate:'turn_completed',usage:{modelUsage:{'grok-5':{inputTokens:1000,cachedReadTokens:200,cacheCreationTokens:100,outputTokens:300,reasoningTokens:120,costUsdTicks:120000000}}}};
  write(path.join(folder,'sessions','old-session','updates.jsonl'),JSON.stringify({jsonrpc:'2.0',method:'session/update',params:{sessionId:'old-session',update,_meta:{eventId:'fixture-grok-history',agentTimestampMs:at}}})+'\n');
}

test('old Grok transcripts are history, never proof of a current sign-in',async t=>{
  const {computer}=await bench(t);
  const box=await computer('GrokHistory',undefined,isolated);
  writeGrokHistory(box.env.GROK_HOME,Date.now()-60_000);
  const call=box.connect(await box.start().ready());
  const history=await call('POST','kuvo/agents/api/kuvo/usage/refresh?zone=UTC',{});
  assert.equal(history.status,200,history.text);
  const grok=history.json.data.rows.find(r=>r.provider==='grok-cli');
  assert.ok(grok?.days.some(d=>d.tokens>0),JSON.stringify(grok));
  const limits=await call('GET','kuvo/agents/api/kuvo/limits');
  assert.equal(limits.status,200,limits.text);
  assert.ok(!limits.json.data.accounts.some(a=>a.provider==='grok-cli'));
  assert.ok(!limits.json.data.rows.some(a=>a.provider==='grok-cli'));
});

test('Insights decodes only changed SQLite transcript projections (Rust writer)',async t=>{
  if(boxBinary().typescript)return t.skip('The Rust transcript writer adds a compatible revision index.');
  const codex=await scriptedCodex();
  t.after(()=>codex.close());
  const id='ses_usagecursor';
  codex.seed(id);
  const listener=await codex.start();
  codex.script('usage cursor',[
    ['thread/tokenUsage/updated',{tokenUsage:{last:{totalTokens:5000,inputTokens:4000,cachedInputTokens:1000,outputTokens:1000,reasoningOutputTokens:200},modelContextWindow:272000}}],
    ['item/completed',{type:'agentMessage',id:'m1',text:'Fixture answer.'}]
  ]);
  await codex.prompt(id,'usage cursor');
  await codex.idles(id);
  const {DatabaseSync}=await import('node:sqlite');
  const index=path.join(codex.home,'agents','usage','insights.sqlite');
  const transcript=path.join(codex.home,'agents','sessions',id,'messages-v1.sqlite');
  const checkpoint=()=>{
    let db;
    try{
      db=new DatabaseSync(index,{readOnly:true});
      const row=db.prepare('SELECT body FROM state WHERE key=?').get('scan/rust-transcript-v1/'+id);
      return row&&JSON.parse(row.body);
    }catch{return null}finally{db?.close();}
  };
  const kept=await until(()=>{
    const c=checkpoint();
    if(!c?.cursor?.revision)return null;
    let source,observed;
    try{
      source=new DatabaseSync(transcript,{readOnly:true});
      observed=new DatabaseSync(index,{readOnly:true});
      const revision=source.prepare('SELECT revision FROM insight_revision WHERE id=1').get().revision;
      const tokens=observed.prepare('SELECT SUM(tokens) AS tokens FROM event WHERE session=? AND human=0').get(id).tokens;
      return c.cursor.revision===revision&&tokens>0&&c;
    }catch{return null}finally{source?.close();observed?.close();}
  },'The completed turn did not reach the index');
  const before=await request(listener,'GET','kuvo/agents/api/kuvo/insights?period=7&zone=UTC');
  assert.equal(before.status,200,before.text);
  const db=new DatabaseSync(transcript);
  let newRevision;
  try{
    assert.equal(db.prepare('PRAGMA user_version').get().user_version,1);
    const rows=db.prepare('SELECT position,insight FROM message WHERE insight IS NOT NULL').all();
    assert.ok(rows.length>=2);
    const row=rows.find(r=>JSON.parse(r.insight).type==='assistant'&&JSON.parse(r.insight).tokens?.input>0);
    assert.ok(row);
    const message=JSON.parse(row.insight);
    message.tokens.input+=13;
    db.prepare('UPDATE message SET insight=? WHERE position=?').run(JSON.stringify(message),row.position);
    newRevision=db.prepare('SELECT revision FROM insight_revision WHERE id=1').get().revision;
    assert.ok(newRevision>kept.cursor.revision);
  }finally{db.close();}
  const after=await until(()=>{const c=checkpoint();return c?.cursor?.revision===newRevision&&c;},'The changed projection did not reach the index');
  assert.equal(after.decoded,1);
  const view=await request(listener,'GET','kuvo/agents/api/kuvo/insights?period=7&zone=UTC');
  assert.equal(view.json.data.totals.tokens,before.json.data.totals.tokens+13);
});

test('native Claude calls replace portable estimates without counting private logs or replays', async t => {
  const fs=await import('node:fs');
  const {computer}=await bench(t);
  const box=await computer('NativeUsage',undefined,isolated);
  const id='ses_native_usage';
  const provider='claude-code@fixture';
  const model={providerID:provider,id:'claude-opus-4-6'};
  const at=Date.now()-5000;
  const entry=(call,input,session='native-thread')=>({timestamp:new Date(at).toISOString(),sessionId:session,requestId:'req-'+call,message:{id:call,model:model.id,usage:{input_tokens:input,output_tokens:10,cache_read_input_tokens:20}}});
  const home=path.join(box.home,'agents','claude','accounts','fixture');
  const log=path.join(home,'projects','project','native.jsonl');
  const row=JSON.stringify(entry('first',100));
  write(path.join(box.home,'agents','accounts.json'),JSON.stringify([{formatVersion:1},{kind:'claude',id:'fixture',label:'Fixture'}]));
  write(path.join(box.home,'agents','sessions',id,'session.json'),JSON.stringify({id,title:'Native fixture',model,agent:'build',projectID:'fixture',time:{created:at-1000,updated:at+1000},location:{directory:box.home}}));
  write(path.join(box.home,'agents','sessions',id,'agent.json'),JSON.stringify({threads:{[provider]:{state:{claudeSession:'native-thread'}}}}));
  write(path.join(box.home,'agents','sessions',id,'messages.json'),JSON.stringify([{id:'step_fixture',type:'assistant',time:{created:at-1000,completed:at+1000},model,agent:'build',tokens:{input:200,output:10,cache:{read:0,write:0}},content:[]}]));
  write(log,row+'\n'+row+'\n'+JSON.stringify(entry('unrelated',999,'another-thread'))+'\n');
  write(path.join(box.env.CLAUDE_CONFIG_DIR,'projects','private.jsonl'),JSON.stringify(entry('private',9999))+'\n');
  let resident=box.start();
  let call=box.connect(await resident.ready());
  const tokens=async()=>{
    const answer=await call('GET','kuvo/agents/api/kuvo/insights/summary?period=7&zone=UTC'+(boxBinary().typescript?'&refresh=1':''));
    assert.equal(answer.status,200,answer.text);
    return answer.json.data.totals.tokens;
  };
  await until(async()=>await tokens()===130,'The native calls did not replace the portable estimate');
  if(boxBinary().typescript)return;
  fs.appendFileSync(log,JSON.stringify(entry('second',30))+'\n');
  await until(async()=>await tokens()===190,'Appended native calls did not reach Insights');
  await resident.stop();
  resident=box.start();
  call=box.connect(await resident.ready());
  await until(async()=>await tokens()===190,'Native calls changed across restart');
  write(log,row+'\n');
  await until(async()=>await tokens()===130,'Rewritten native log left stale calls in Insights');
});

test('two computers keep one account ring and never replace it with a peer’s older reset',{timeout:120000},async t=>{
  if(boxBinary().typescript)return t.skip('The fixture exercises the Rust direct provider reader and correction.');
  const http=await import('node:http');
  const now=Date.now();
  const reset=Math.floor(now/1000)+7200;
  let first=17;
  const provider=http.createServer((req,res)=>{
    const isFirst=req.headers.authorization==='Bearer fixture-first';
    const body={plan_type:'plus',rate_limit:{primary_window:{used_percent:isFirst?first:99,limit_window_seconds:18000,reset_at:isFirst?reset:reset-3600}}};
    res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify(body));
  });
  await new Promise(resolve=>provider.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{provider.close(resolve);provider.closeAllConnections();}));
  const {computer,leases}=await bench(t);
  const env=folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+provider.address().port+'/'});
  const studio=await computer('UsageStudio',undefined,env);
  const lab=await computer('UsageLab',undefined,env);
  const sign=access=>[{integrationID:'openai',active:true,value:{type:'oauth',access,expires:now+3600000,metadata:{accountID:'same-fixture-account'}}}];
  studio.engine.credentials=sign('fixture-first');
  lab.engine.credentials=sign('fixture-second');
  const base='kuvo/agents/api/kuvo/limits';
  const main=studio.connect(await studio.start().ready());
  const refresh=async call=>{
    const response=await call('POST',base+'/refresh',{});
    assert.equal(response.status,200,response.text);
    return until(async()=>{const reply=await call('GET',base);return !reply.json.data.refreshPending&&reply.json.data.accounts.some(a=>a.provider==='openai'&&a.windows.length)&&reply.json.data;},'Provider limits did not settle',20000,100);
  };
  const original=(await refresh(main)).rows.find(row=>row.provider==='openai');
  assert.equal(original.rings.short.used,0.17);
  const setup=await main('GET','kuvo/setup');
  const invite=await main('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});
  assert.equal(invite.status,201,invite.text);
  await until(()=>leases.leaseFor(setup.json.setup),'The main did not claim its relay lease');
  const joined=await lab.join(invite.json.code);
  assert.equal(joined.code,0,joined.log);
  let resident=lab.start();
  let call=lab.connect(await resident.ready());
  const view=async call=>(await call('GET',base)).json.data;
  await until(async()=>{const data=await view(call);return data.rows.find(r=>r.id===original.id)?.rings.short?.used===0.17;},'Lab did not receive Studio’s limit reading',30000,250);
  await refresh(call);
  let last;
  const agrees=async call=>{
    const data=last=await view(call);
    const rows=data.rows.filter(r=>r.id===original.id);
    return rows.length===1&&rows[0].boxes.length===2&&rows[0].rings.short?.used===0.17&&rows[0].rings.short.resetsAt===reset*1000&&rows[0];
  };
  assert.ok(await until(()=>agrees(main),()=> 'Studio did not keep one accepted window: '+JSON.stringify(last),10000,250));
  assert.ok(await until(()=>agrees(call),()=> 'Lab did not merge the account consistently: '+JSON.stringify(last),10000,250));
  first=18;
  await refresh(main);
  await until(async()=>{const data=await view(call);return data.rows.find(r=>r.id===original.id)?.rings.short?.used===0.18;},'Fresh peer reading did not reach Lab',30000,250);
  await resident.stop();
  resident=lab.start();
  call=lab.connect(await resident.ready());
  await until(async()=>{const data=await view(call);return data.rows.find(r=>r.id===original.id)?.rings.short?.used===0.18;},'Peer reading did not survive restart',30000,250);
});

test('Insights watches the engine SQLite channel and counts only new or changed calls',async t=>{
  const fs=await import('node:fs');
  const {DatabaseSync}=await import('node:sqlite');
  const {computer}=await bench(t);
  const box=await computer('EngineUsage',undefined,isolated);
  const folder=path.join(box.home,'data','opencode');
  fs.mkdirSync(folder,{recursive:true});
  const source=path.join(folder,'opencode.db');
  const now=Date.now();
  let db=new DatabaseSync(source);
  db.exec('PRAGMA journal_mode=WAL; CREATE TABLE session_message(id TEXT PRIMARY KEY, session_id TEXT, type TEXT, time_created INTEGER,time_updated INTEGER,data TEXT); CREATE TABLE session_v2(id TEXT PRIMARY KEY,title TEXT,parent_id TEXT,metadata TEXT); CREATE INDEX message_updated ON session_message(time_updated)');
  db.prepare('INSERT INTO session_v2 VALUES (?,?,?,?)').run('ses_engine_usage','Engine usage fixture',null,'{}');
  for(let i=0;i<20;i++)db.prepare('INSERT INTO session_v2 VALUES (?,?,?,?)').run('ses_inactive_'+i,'Inactive session',null,'{}');
  const insert=(id,at,input,output)=>{
    db.prepare('INSERT OR REPLACE INTO session_message VALUES (?, ?, ?, ?, ?, ?)').run(id,'ses_engine_usage','assistant',at,Date.now(),JSON.stringify({time:{completed:at},model:{providerID:'opencode-go',id:'claude-opus-4-6'},tokens:{input,output,cache:{read:10,write:2}},cost:0.013}));
  };
  insert('old',now-5*3600000,100,10);
  insert('recent',now-2*3600000,30,5);
  // Seed the first watermark well before the new call, as the maker's old history was written.
  db.prepare('UPDATE session_message SET time_updated=time_created').run();
  db.close();
  let resident=box.start();
  let call=box.connect(await resident.ready());
  const totals=async()=>{
    const reply=await call('GET','kuvo/agents/api/kuvo/insights/summary?period=7&zone=UTC'+(boxBinary().typescript?'&refresh=1':''));
    assert.equal(reply.status,200,reply.text);
    return reply.json.data.totals;
  };
  await until(async()=>(await totals()).tokens===169,'Engine SQLite usage did not reach Insights');
  const spend=await call('POST','kuvo/agents/api/kuvo/usage/refresh?zone=UTC',{});
  assert.equal(spend.status,200,spend.text);
  assert.equal(spend.json.data.rows.find(row=>row.provider==='opencode-go')?.estimated,false);
  assert.equal(spend.json.data.totals.opencode.reduce((sum,day)=>sum+day.tokens,0),169);
  assert.equal(spend.json.data.totals.opencode.reduce((sum,day)=>sum+day.cost,0),0.026);
  if(boxBinary().typescript)return;
  db=new DatabaseSync(source);
  try{
    insert('new',now-100,20,5);
    let last; await until(async()=>{last=await totals();return last.tokens===206;},()=> {const kept=new DatabaseSync(path.join(box.home,'agents','usage','insights.sqlite'),{readOnly:true});try{return 'New engine call did not reach Insights\n'+JSON.stringify(last)+'\n'+JSON.stringify(kept.prepare("SELECT key,body FROM state WHERE key LIKE 'scan/rust-engine-v1/%'").all())+'\n'+resident.log();}finally{kept.close();}});
    const index=path.join(box.home,'agents','usage','insights.sqlite');
    const kept=new DatabaseSync(index,{readOnly:true});
    try{
      const decoded=kept.prepare("SELECT body FROM state WHERE key LIKE 'scan/rust-engine-v1/%/decoded'").get();
      assert.equal(JSON.parse(decoded.body),2,'The recent overlap and new call are read; older calls are not decoded');
      const cursor=kept.prepare("SELECT body FROM state WHERE key LIKE 'scan/rust-engine-v1/%' AND key NOT LIKE '%/decoded'").get();
      assert.equal(JSON.parse(cursor.body).headers,1,'Only affected session headers and their lineage are decoded');
    }finally{kept.close();}
    insert('new',now-100,20,8);
    await until(async()=>(await totals()).tokens===209,'Changed engine call remained stale');
  }finally{db.close();}
  await resident.stop();
  resident=box.start();
  call=box.connect(await resident.ready());
  await until(async()=>(await totals()).tokens===209,'Engine counts changed across restart');
});

test('Antigravity generation metadata supplies native calls without reading conversation bodies',async t=>{
  const fs=await import('node:fs');
  const {DatabaseSync}=await import('node:sqlite');
  const {computer}=await bench(t);
  const box=await computer('AntigravityUsage',undefined,isolated);
  const id='ses_antigravity_usage', native='native-ag';
  const at=Math.floor((Date.now()-5000)/1000)*1000;
  const model={providerID:'antigravity',id:'gemini-3.6-flash'};
  const varint=n=>{const out=[];while(n>=128){out.push((n%128)|128);n=Math.floor(n/128);}out.push(n);return Buffer.from(out);};
  const integer=(field,value)=>Buffer.concat([varint(field*8),varint(value)]);
  const bytes=(field,value)=>Buffer.concat([varint(field*8+2),varint(value.length),value]);
  const generation=input=>bytes(1,Buffer.concat([
    bytes(19,Buffer.from(model.id)),
    bytes(4,Buffer.concat([integer(1,100),integer(2,input),integer(3,200),integer(5,50)])),
    bytes(9,bytes(4,integer(1,at/1000)))
  ]));
  const folder=path.join(box.home,'agents','antigravity','profile','antigravity','conversations');
  fs.mkdirSync(folder,{recursive:true});
  const source=path.join(folder,native+'.db');
  let db=new DatabaseSync(source);
  db.exec('CREATE TABLE gen_metadata(idx INTEGER PRIMARY KEY, data BLOB)');
  db.prepare('INSERT INTO gen_metadata VALUES (?,?)').run(1,generation(1000));
  db.close();
  const session=path.join(box.home,'agents','sessions',id);
  write(path.join(session,'session.json'),JSON.stringify({id,title:'Antigravity fixture',model,agent:'build',projectID:'fixture',time:{created:at-1000,updated:at+500},location:{directory:box.home}}));
  write(path.join(session,'agent.json'),JSON.stringify({threads:{antigravity:{state:{acpSession:native}}}}));
  write(path.join(session,'messages.json'),JSON.stringify([{id:'step_fixture',type:'assistant',time:{created:at-1000,completed:at+500},model,agent:'build',content:[]}]));
  const resident=box.start();
  const call=box.connect(await resident.ready());
  const tokens=async()=>{
    const response=await call('GET','kuvo/agents/api/kuvo/insights/summary?period=7&zone=UTC'+(boxBinary().typescript?'&refresh=1':''));
    assert.equal(response.status,200,response.text);
    return response.json.data.totals.tokens;
  };
  await until(async()=>await tokens()===1350,'Antigravity native generation did not reach Insights');
  if(boxBinary().typescript)return;
  db=new DatabaseSync(source);
  try{
    db.prepare('INSERT INTO gen_metadata VALUES (?,?)').run(2,generation(50));
    let last;await until(async()=>{last=await tokens();return last===1750;},()=>{const kept=new DatabaseSync(path.join(box.home,'agents','usage','insights.sqlite'),{readOnly:true});try{return 'New Antigravity generation did not reach Insights\n'+last+'\n'+JSON.stringify(kept.prepare('SELECT * FROM native_file').all())+'\n'+resident.log();}finally{kept.close();}});
  }finally{db.close();}
});

test('Antigravity history reads only usable generation stores, follows WAL changes and forgets removed databases',async t=>{
  const fs=await import('node:fs');
  const {DatabaseSync}=await import('node:sqlite');
  const {computer}=await bench(t);
  const box=await computer('AntigravityStores',undefined,isolated);
  const at=Math.floor((Date.now()-5000)/1000)*1000,model='gemini-3.6-flash';
  const varint=n=>{const out=[];while(n>=128){out.push((n%128)|128);n=Math.floor(n/128);}out.push(n);return Buffer.from(out);};
  const integer=(field,value)=>Buffer.concat([varint(field*8),varint(value)]);
  const bytes=(field,value)=>Buffer.concat([varint(field*8+2),varint(value.length),value]);
  const generation=(input,embedded=true)=>bytes(1,Buffer.concat([
    bytes(19,Buffer.from(model)),bytes(4,Buffer.concat([integer(2,input),integer(3,10)])),
    ...(embedded?[bytes(9,bytes(4,integer(1,at/1000)))]:[])
  ]));
  const folder=path.join(box.home,'agents','antigravity','profile','antigravity-acp','conversations');
  fs.mkdirSync(folder,{recursive:true});
  const store=(name,rows=[],wal=false)=>{
    const db=new DatabaseSync(path.join(folder,name+'.db'));
    if(wal)db.exec('PRAGMA journal_mode=WAL');
    db.exec('CREATE TABLE gen_metadata(idx INTEGER PRIMARY KEY, data BLOB)');
    for(const [i,data] of rows.entries())db.prepare('INSERT INTO gen_metadata VALUES (?,?)').run(i+1,data);
    return db;
  };
  store('first',[generation(100)]).close();
  const live=store('live',[generation(50)],true);t.after(()=>{try{live.close();}catch{}});
  store('legacy',[generation(20,false)]).close();
  const empty=new DatabaseSync(path.join(folder,'empty.db'));empty.exec('CREATE TABLE unrelated(id INTEGER)');empty.close();
  write(path.join(folder,'broken.db'),'not SQLite');
  store('oversized',[Buffer.alloc(1048577)]).close();
  if(process.platform!=='win32')fs.symlinkSync(path.join(folder,'first.db'),path.join(folder,'same.db'));
  write(path.join(box.home,'agents','usage','pricing','litellm.json'),JSON.stringify({models:{[model]:{i:1,o:2,cw:1,cr:0.5}}}));
  let resident=box.start(),call=box.connect(await resident.ready());
  const base='kuvo/agents/api/kuvo/usage';
  const refresh=async()=>{const res=await call('POST',base+'/refresh?zone=UTC',{});assert.equal(res.status,200,res.text);};
  const read=async()=>{const res=await call('GET',base+'?zone=UTC');assert.equal(res.status,200,res.text);return res.json.data.totals.antigravity.reduce((sum,d)=>sum+d.tokens,0);};
  const settle=async n=>until(async()=>{const seen=await read();return seen===n;},'Antigravity stores did not settle to '+n);
  await refresh();await settle(170);
  live.prepare('INSERT INTO gen_metadata VALUES (?,?)').run(2,generation(30));
  await refresh();await settle(210);
  const legacy=new DatabaseSync(path.join(folder,'legacy.db'));
  legacy.exec('CREATE TABLE steps(idx INTEGER PRIMARY KEY, metadata BLOB)');
  legacy.prepare('INSERT INTO steps VALUES (?,?)').run(1,bytes(1,integer(1,at/1000)));legacy.close();
  await refresh();await settle(240);
  fs.rmSync(path.join(folder,'broken.db'));store('broken',[generation(5)]).close();
  await refresh();await settle(255);
  fs.rmSync(path.join(folder,'first.db'));
  if(process.platform!=='win32')fs.rmSync(path.join(folder,'same.db'));
  await refresh();await settle(145);
  live.close();await resident.stop();resident=box.start();call=box.connect(await resident.ready());
  await refresh();await settle(145);
});

test('OpenCode Go limits read the engine’s saved credentials without starting the engine',async t=>{
  if(boxBinary().typescript)return t.skip('The Rust box reads the engine’s local credential database.');
  const fs=await import('node:fs');
  const http=await import('node:http');
  const {DatabaseSync}=await import('node:sqlite');
  const now=Date.now();
  let requests=0;
  const maker=http.createServer((request,response)=>{
    if(request.url==='/opencodeGo'&&request.headers.authorization==='Bearer fixture-go-db')requests++;
    response.writeHead(200,{'Content-Type':'application/json'});
    response.end(JSON.stringify({usage:{rolling:{percent:22,resetsAt:new Date(now+7200000).toISOString()}}}));
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('GoDatabase',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const folder=path.join(box.home,'data','opencode');
  fs.mkdirSync(folder,{recursive:true});
  const sqlite=path.join(folder,'opencode.db');
  const db=new DatabaseSync(sqlite);
  db.exec('CREATE TABLE credential(id TEXT, integration_id TEXT, value TEXT, active INTEGER, time_updated INTEGER)');
  db.prepare('INSERT INTO credential VALUES (?,?,?,?,?)').run('one','opencode-go',JSON.stringify({type:'api',key:'fixture-go-db'}),1,now);
  db.close();
  const before=fs.readFileSync(sqlite);
  // The stand-in engine stays out of the route. The box has no active engine
  // rendezvous and cannot manufacture a different reading from an HTTP stub.
  const engineRecord=path.join(box.home,'run','engine.json');
  fs.unlinkSync(engineRecord);
  const call=box.connect(await box.start().ready());
  const answer=await call('POST','kuvo/agents/api/kuvo/limits/refresh',{});
  assert.equal(answer.status,200,answer.text);
  await until(async()=>{
    const reply=await call('GET','kuvo/agents/api/kuvo/limits');
    return reply.json.data.accounts.find(a=>a.provider==='opencode-go')?.windows[0]?.used===0.22;
  },'Saved OpenCode Go credentials did not provide a quota reading');
  assert.equal(requests,1);
  assert.equal(box.engine.asked.filter(r=>r.path==='/api/credential').length,0);
  assert.equal(fs.existsSync(engineRecord),false);
  assert.deepEqual(fs.readFileSync(sqlite),before,'The maker database remained read-only');
  const signedOutDb=new DatabaseSync(sqlite);
  signedOutDb.exec('DELETE FROM credential');signedOutDb.close();
  const signedOut=await call('GET','kuvo/agents/api/kuvo/limits');
  assert.ok(!signedOut.json.data.accounts.some(a=>a.provider==='opencode-go'));
  assert.equal(requests,1,'A sign-out is a local credential check, not a maker request');
});

test('empty usage history retains service order on both boxes', async t=>{
  const {computer}=await bench(t);
  const box=await computer('UsageOrder',undefined,isolated);
  const call=box.connect(await box.start().ready());
  const response=await call('POST','kuvo/agents/api/kuvo/usage/refresh?zone=UTC',{});
  assert.equal(response.status,200,response.text);
  const rows=response.json.data.rows;
  assert.deepEqual(rows.map(r=>r.provider),['claude-code','codex-cli','grok-cli','antigravity']);
  assert.ok(rows.every(r=>r.boxes[0].here&&r.scanned>0&&r.days.length===0));
});

test('native usage history is deduplicated, priced and assigned to the client’s calendar day',async t=>{
  const {computer}=await bench(t);
  const box=await computer('HistoryZone',undefined,isolated);
  const now=new Date();
  const at=Date.UTC(now.getUTCFullYear(),now.getUTCMonth(),now.getUTCDate()-1,23,30);
  const folder=path.join(box.home,'agents','claude','accounts','fixture','projects','project');
  write(path.join(box.home,'agents','accounts.json'),JSON.stringify([{formatVersion:1},{kind:'claude',id:'fixture',label:'Fixture'}]));
  const model='claude-opus-4-6';
  const row={timestamp:new Date(at).toISOString(),sessionId:'native',requestId:'first',costUSD:1.25,message:{id:'first',model,usage:{input_tokens:100,output_tokens:20,cache_creation_input_tokens:10,cache_read_input_tokens:200}}};
  const unknown={timestamp:new Date(at).toISOString(),sessionId:'native',requestId:'unknown',message:{id:'unknown',model:'fixture-unknown-model-123',usage:{input_tokens:10,output_tokens:10}}};
  write(path.join(folder,'account.jsonl'),[row,row,unknown].map(JSON.stringify).join('\n')+'\n');
  const call=box.connect(await box.start().ready());
  const base='kuvo/agents/api/kuvo/usage';
  const refresh=await call('POST',base+'/refresh?zone=UTC',{});
  assert.equal(refresh.status,200,refresh.text);
  const reading=async zone=>{
    const response=await call('GET',base+'?zone='+zone);
    assert.equal(response.status,200,response.text);
    return response.json.data.rows.find(r=>r.provider==='claude-code@fixture');
  };
  const utc=await until(async()=>{const row=await reading('UTC');return row?.days?.length&&row;},'Native usage was not read');
  assert.equal(utc.days.length,1);
  assert.deepEqual(utc.days[0],{date:new Date(at).toISOString().slice(0,10),tokens:330,cost:1.25,models:[{model,tokens:330,cost:1.25}],unknown:['fixture-unknown-model-123']});
  const istanbul=await reading('Europe/Istanbul');
  assert.deepEqual(istanbul.days[0],{...utc.days[0],date:new Date(at+3*3600000).toISOString().slice(0,10)});
});

test('Codex fallback pricing uses the saved counts without reparsing its logs',async t=>{
  const {computer}=await bench(t);
  const box=await computer('FallbackUsage',undefined,isolated);
  const at=new Date(Date.now()-5000).toISOString();
  const file=path.join(box.env.CODEX_HOME,'sessions','rollout.jsonl');
  const model='fixture-future-codex-model-123';
  write(file,[
    {type:'session_meta',timestamp:at,payload:{id:'fixture-thread'}},
    {type:'turn_context',payload:{model}},
    {type:'event_msg',timestamp:at,payload:{type:'token_count',info:{total_token_usage:{input_tokens:100,cached_input_tokens:20,output_tokens:25,total_tokens:125}}}}
  ].map(JSON.stringify).join('\n')+'\n');
  const call=box.connect(await box.start().ready());
  const base='kuvo/agents/api/kuvo/usage';
  assert.equal((await call('POST',base+'/refresh?zone=UTC',{})).status,200);
  const read=async()=>{
    const answer=await call('GET',base+'?zone=UTC');
    assert.equal(answer.status,200,answer.text);
    return answer.json.data;
  };
  const before=await until(async()=>{const data=await read();return data.rows.find(r=>r.provider==='codex-cli')?.days[0]?.unknown?.includes(model)&&data;},'The unknown Codex model was not reported');
  assert.equal(before.totals.codex[0].tokens,0);
  const fallback=before.pricing.codex.choices[0];
  assert.ok(fallback);
  const chosen=await call('PUT',base+'/pricing?zone=UTC',{codex:{fallback}});
  assert.equal(chosen.status,200,chosen.text);
  assert.equal(chosen.json.data.totals.codex[0].tokens,125);
  assert.ok(chosen.json.data.totals.codex[0].cost>0);
  assert.deepEqual(chosen.json.data.totals.codex[0].fallback,[fallback]);
  assert.ok(chosen.json.data.totals.codex[0].unknown.includes(model));
  assert.deepEqual(chosen.json.data.pricing.codex.fallback,fallback);
  const none=await call('PUT',base+'/pricing?zone=UTC',{codex:{fallback:null}});
  assert.equal(none.status,200,none.text);
  assert.equal(none.json.data.totals.codex[0].tokens,0);
});

test('a changed price feed reprices saved history and a bad feed does not replace a good cache',async t=>{
  if(boxBinary().typescript)return t.skip('The Rust isolated price-feed endpoint fixture is loopback-only.');
  const http=await import('node:http');
  const fs=await import('node:fs');
  const requests=[];
  const model='fixture-price-feed-model';
  const maker=http.createServer((req,res)=>{
    requests.push([req.url,req.headers['if-none-match']]);
    if(req.url==='/litellm'){
      res.writeHead(200,{'Content-Type':'application/json','etag':'new'});
      res.end(JSON.stringify({[model]:{input_cost_per_token:0.000002,output_cost_per_token:0.000004}}));
    }else if(req.url==='/models_dev'){
      res.writeHead(304);res.end();
    }else{
      res.writeHead(200,{'Content-Type':'application/json'});res.end('{"invalid":true}');
    }
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('PriceFeed',undefined,folder=>({...isolated(folder),KUVO_TEST_PRICE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const pricing=path.join(box.home,'agents','usage','pricing');
  const kept=JSON.stringify({models:{'fixture-kept':{i:1,o:1,cw:1,cr:1}}});
  write(path.join(pricing,'models_dev.json'),kept);
  const now=Date.now(), old=now-7200000;
  write(path.join(pricing,'state.json'),JSON.stringify({litellm:{fetchedAt:old},models_dev:{fetchedAt:old,etag:'kept'},supplement:{fetchedAt:old}}));
  write(path.join(box.env.CLAUDE_CONFIG_DIR,'projects','fixture.jsonl'),JSON.stringify({timestamp:new Date(now-5000).toISOString(),sessionId:'fixture',message:{id:'one',model,usage:{input_tokens:1000,output_tokens:500}}})+'\n');
  let resident=box.start();
  let call=box.connect(await resident.ready());
  const base='kuvo/agents/api/kuvo/usage?zone=UTC';
  const value=async()=>{
    const r=await call('GET',base);assert.equal(r.status,200,r.text);
    return r.json.data.totals.claude?.[0];
  };
  const priced=await until(async()=>{const day=await value();return day?.tokens===1500&&day;},'New feed rates did not reach saved usage');
  assert.equal(priced.cost,0.004);
  await until(()=>requests.length===3&&JSON.parse(fs.readFileSync(path.join(pricing,'state.json'),'utf8')).supplement?.failedAt,'Feed round state not saved');
  assert.ok(requests.some(([url,etag])=>url==='/models_dev'&&etag==='kept'));
  assert.equal(fs.readFileSync(path.join(pricing,'models_dev.json'),'utf8'),kept);
  assert.equal(fs.existsSync(path.join(pricing,'supplement.json')),false);
  const before=requests.length;
  await resident.stop();
  resident=box.start();call=box.connect(await resident.ready());
  await until(async()=>{const day=await value();return day?.cost===0.004;},'Saved feed prices were not restored');
  assert.equal(requests.length,before,'The saved hourly freshness survives restart');
});

test('usage and Insights merge only admitted peer records, retain them across restart and filter by computer',{timeout:120000},async t=>{
  const fs=await import('node:fs');
  const {DatabaseSync}=await import('node:sqlite');
  const {computer,leases}=await bench(t);
  const studio=await computer('HistoryStudio',undefined,isolated);
  const other=process.env.KUVO_USAGE_PEER_BIN?boxBinary(process.env.KUVO_USAGE_PEER_BIN):undefined;
  const lab=await computer('HistoryLab',other,isolated);
  const now=Date.now()-5000;
  const seed=(box,session,input,billed)=>{
    const folder=path.join(box.home,'data','opencode');
    fs.mkdirSync(folder,{recursive:true});
    const db=new DatabaseSync(path.join(folder,'opencode.db'));
    try{
      db.exec('CREATE TABLE session_message(id TEXT PRIMARY KEY, session_id TEXT, type TEXT, time_created INTEGER,time_updated INTEGER,data TEXT); CREATE TABLE session_v2(id TEXT PRIMARY KEY,title TEXT,parent_id TEXT,metadata TEXT)');
      db.prepare('INSERT INTO session_v2 VALUES (?,?,?,?)').run(session,session,null,'{}');
      db.prepare('INSERT INTO session_message VALUES (?,?,?,?,?,?)').run(session+'-call',session,'assistant',now,now,JSON.stringify({time:{completed:now},model:{providerID:'opencode-go',id:'claude-opus-4-6'},tokens:{input,output:10,cache:{read:5,write:0}},cost:billed}));
    }finally{db.close();}
  };
  seed(studio,'ses_studio_history',100,0.01);
  // A restored transcript from an unadmitted origin may be present, but must
  // not become local or be forwarded to another computer as eligible usage.
  const foreign='ses_foreign_history',foreignFolder=path.join(studio.home,'agents','sessions',foreign);
  const foreignModel={providerID:'claude-code',id:'claude-opus-4-6'};
  write(path.join(foreignFolder,'session.json'),JSON.stringify({id:foreign,title:'Unadmitted origin',model:foreignModel,agent:'build',projectID:'fixture',time:{created:now,updated:now},location:{directory:studio.home}}));
  write(path.join(foreignFolder,'messages.json'),JSON.stringify([{id:'foreign-call',type:'assistant',time:{created:now,completed:now,origin:'not-an-admitted-computer'},model:foreignModel,agent:'build',tokens:{input:99999,output:9,cache:{read:0,write:0}},content:[]}]));
  const first=studio.start();
  const main=studio.connect(await first.ready());
  const setup=await main('GET','kuvo/setup');
  const invite=await main('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});
  assert.equal(invite.status,201,invite.text);
  await until(()=>leases.leaseFor(setup.json.setup),'History Studio did not claim its relay lease');
  const joined=await lab.join(invite.json.code);
  assert.equal(joined.code,0,joined.log);
  seed(lab,'ses_lab_history',40,0.02);
  let resident=lab.start();
  let view=lab.connect(await resident.ready());
  const usage='kuvo/agents/api/kuvo/usage';
  const summary='kuvo/agents/api/kuvo/insights/summary?period=7&zone=UTC';
  const history=async call=>{
    const response=await call('GET',usage+'?zone=UTC');
    assert.equal(response.status,200,response.text);
    const hosted=response.json.data.totals.opencode;
    return hosted.reduce((sum,day)=>sum+day.tokens,0);
  };
  const insight=async(call,filter='')=>{
    const response=await call('GET',summary+filter);
    assert.equal(response.status,200,response.text);
    return response.json.data;
  };
  // The signed setup and each usage service's own directory are separate
  // handshakes on the TS oracle. Wait for the directory, not a wall-clock delay.
  const known=async call=>(await call('GET','kuvo/agents/api/kuvo/insights?period=7&zone=UTC')).json?.data?.computers??[];
  await until(async()=>(await known(main)).some(c=>c.name==='HistoryLab')&&(await known(view)).some(c=>c.name==='HistoryStudio'),'The usage directories did not learn both computers',30000,250);
  // An explicit refresh is allowed to read now; the later updates and kept
  // copy must be served without another refresh.
  await main('POST',usage+'/refresh?zone=UTC',{});
  await view('POST',usage+'/refresh?zone=UTC',{});
  await main('GET',summary+'&refresh=1');
  await view('GET',summary+'&refresh=1');
  let mainTokens,labTokens;
  await until(async()=>{mainTokens=await history(main);labTokens=await history(view);return mainTokens===170&&labTokens===170;},()=> 'Peer history did not merge: '+mainTokens+' / '+labTokens+'\n'+first.log()+'\n'+resident.log(),30000,250);
  const billed=await main('GET',usage+'?zone=UTC');
  assert.ok(Math.abs(billed.json.data.totals.opencode.reduce((sum,day)=>sum+day.cost,0)-0.03)<1e-12);
  assert.ok(billed.json.data.rows.filter(row=>row.provider==='opencode-go').every(row=>row.estimated===false));
  const combined=await until(async()=>{
    const a=await insight(main),b=await insight(view);
    return a.totals.tokens===170&&b.totals.tokens===170&&a.lists.find(l=>l.key==='computers').rows.length===2&&a;
  },'Peer Insight totals or computer rows did not merge',30000,250);
  const computers=combined.lists.find(l=>l.key==='computers').rows;
  const key=computers.find(c=>c.name==='HistoryLab')?.id;
  assert.ok(key,JSON.stringify(computers));
  assert.equal((await insight(main,'&computer='+encodeURIComponent(key))).totals.tokens,55);
  assert.equal((await insight(main,'&computer=unknown')).totals.tokens,0);
  let kept=170;
  if(!boxBinary().typescript&&!other?.typescript){
    // A new call must be sent from the retained cursor and recent history slots,
    // not wait for another full history scan or client query.
    const source=path.join(studio.home,'data','opencode','opencode.db');
    const change=new DatabaseSync(source);
    try{
      const at=Date.now()-100;
      change.prepare('INSERT INTO session_message VALUES (?,?,?,?,?,?)').run('studio-new','ses_studio_history','assistant',at,Date.now(),JSON.stringify({time:{completed:at},model:{providerID:'opencode-go',id:'claude-opus-4-6'},tokens:{input:30,output:10,cache:{read:5,write:0}},cost:0.003}));
    }finally{change.close();}
    await until(async()=>await history(view)===215&&(await insight(view)).totals.tokens===215,()=> 'Peer live history delta or Insight cursor did not advance\n'+first.log()+'\n'+resident.log(),30000,250);
    kept=215;
  }
  await first.stop();
  await resident.stop();
  resident=lab.start(); view=lab.connect(await resident.ready());
  await until(async()=>await history(view)===kept&&(await insight(view)).totals.tokens===kept,'Kept peer history or Insights went missing after restart',30000,250);
});

test('Insights groups model aliases and ranks named sessions from indexed usage',async t=>{
  const fs=await import('node:fs');
  const {DatabaseSync}=await import('node:sqlite');
  const {computer}=await bench(t);
  const box=await computer('NamedInsights',undefined,isolated);
  const now=Date.now()-5000;
  const folder=path.join(box.home,'data','opencode');
  fs.mkdirSync(folder,{recursive:true});
  const db=new DatabaseSync(path.join(folder,'opencode.db'));
  const models=['claude-opus-4-6','anthropic/claude-opus-4.6','claude-opus-4-6-latest','claude-opus-4.6-20261101'];
  try{
    db.exec('CREATE TABLE session_message(id TEXT PRIMARY KEY, session_id TEXT, type TEXT, time_created INTEGER,time_updated INTEGER,data TEXT); CREATE TABLE session_v2(id TEXT PRIMARY KEY,title TEXT,parent_id TEXT,metadata TEXT)');
    for(let i=0;i<models.length;i++){
      const id='ses_named'+i;
      db.prepare('INSERT INTO session_v2 VALUES (?,?,?,?)').run(id,'Named session '+i,null,'{}');
      db.prepare('INSERT INTO session_message VALUES (?,?,?,?,?,?)').run(id+'-call',id,'assistant',now,now,JSON.stringify({time:{completed:now},model:{providerID:'opencode-go',id:models[i]},tokens:{input:100*(i+1),output:10,cache:{read:5,write:0}},cost:0.001}));
    }
    const prior=now-8*86400000;
    db.prepare('INSERT INTO session_v2 VALUES (?,?,?,?)').run('ses_named_prior','Prior session',null,'{}');
    db.prepare('INSERT INTO session_message VALUES (?,?,?,?,?,?)').run('prior-call','ses_named_prior','assistant',prior,prior,JSON.stringify({time:{completed:prior},model:{providerID:'opencode-go',id:models[0]},tokens:{input:100,output:10,cache:{read:5,write:0}},cost:0.001}));
  }finally{db.close();}
  const call=box.connect(await box.start().ready());
  const base='kuvo/agents/api/kuvo/insights/summary?period=7&zone=UTC';
  const read=async filter=>{
    const reply=await call('GET',base+filter+(boxBinary().typescript?'&refresh=1':''));
    assert.equal(reply.status,200,reply.text);
    return reply.json.data;
  };
  const summary=await until(async()=>{const value=await read('');return value.totals.tokens===1060&&value;},'Alias usage was not counted');
  const modelsRows=summary.lists.find(list=>list.key==='models').rows;
  assert.equal(modelsRows.length,1);
  assert.deepEqual({id:modelsRows[0].id,name:modelsRows[0].name,tokens:modelsRows[0].tokens},{id:'opus-4.6',name:'Opus 4.6',tokens:1060});
  assert.equal(summary.lists.find(list=>list.key==='providers').rows[0].name,'OpenCode Go');
  assert.equal(summary.lists.find(list=>list.key==='bots').rows.length,4);
  assert.ok(Math.abs(summary.totals.cacheRate-20/1020)<1e-12);
  assert.ok(Math.abs(summary.change.tokens-(1060-115)/115)<1e-12);
  assert.ok(summary.standouts.some(row=>row.kind==='cost'&&row.reason==='Highest cost'));
  assert.equal((await read('&model=opus-4.6')).totals.tokens,1060);
  assert.equal((await read('&model=unknown')).totals.tokens,0);
});

test('Cursor keeps complete account-wide UTC days, reprices saved counts and refuses an incomplete page',async t=>{
  if(boxBinary().typescript)return t.skip('The TypeScript scratch bridge disables Cursor maker calls.');
  const fs=await import('node:fs');
  const http=await import('node:http');
  const {DatabaseSync}=await import('node:sqlite');
  const at=Date.now(),day=86400000,today=Math.floor(at/day)*day;
  const access=['e30',Buffer.from(JSON.stringify({sub:'workos|cursor-history-fixture',exp:Math.floor(at/1000)+3600})).toString('base64url'),'fixture'].join('.');
  const asked=[],model='fixture-cursor-history-model';
  let failToday=false;
  const server=http.createServer((request,response)=>{
    const send=value=>{response.writeHead(200,{'Content-Type':'application/json'});response.end(JSON.stringify(value));};
    if(request.url==='/cursor/exchange'){assert.equal(request.headers.authorization,'Bearer fixture-cursor-history');send({accessToken:access});return;}
    if(request.url==='/cursor/GetCurrentPeriodUsage'){send({enabled:true,planUsage:{limit:100,totalPercentUsed:12}});return;}
    if(request.url==='/cursor/GetPlanInfo'){send({planInfo:{planName:'pro'}});return;}
    if(request.url==='/cursor/dashboard/get-filtered-usage-events'){
      let body='';request.on('data',chunk=>body+=chunk);
      request.on('end',()=>{
        const given=JSON.parse(body),start=Number(given.startDate);
        asked.push(start);assert.equal(request.headers.cookie,'WorkosCursorSessionToken=cursor-history-fixture%3A%3A'+access);
        if(start!==today&&start!==today-day){send({totalUsageEventsCount:0,usageEventsDisplay:[]});return;}
        const event={timestamp:String(start+1000),model,tokenUsage:{inputTokens:100,outputTokens:10,cacheReadTokens:20,cacheWriteTokens:5}};
        if(start===today&&failToday){
          send({totalUsageEventsCount:2,usageEventsDisplay:given.page===1?[event]:[]});return;
        }
        send({totalUsageEventsCount:1,usageEventsDisplay:[event]});
      });
      return;
    }
    send({});
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{server.close(resolve);server.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('CursorHistory',undefined,folder=>({...isolated(folder),KUVO_TEST_CURSOR_HISTORY:'1',KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+server.address().port+'/'}));
  write(path.join(box.home,'agents','cursor','auth.json'),JSON.stringify({apiKey:'fixture-cursor-history'}));
  const catalog=path.join(box.home,'agents','usage','pricing','litellm.json');
  const rates=multiplier=>JSON.stringify({models:{[model]:{i:multiplier,o:2*multiplier,cw:multiplier,cr:0.1*multiplier}}});
  write(catalog,rates(1));
  let resident=box.start();let view=box.connect(await resident.ready());
  const read=async()=>{const response=await view('GET','kuvo/agents/api/kuvo/usage?zone=UTC');assert.equal(response.status,200,response.text);return response.json.data.rows.find(row=>row.provider==='cursor');};
  const first=await until(async()=>{const row=await read();return row?.days.reduce((total,d)=>total+d.tokens,0)===270&&row;},'The account-wide Cursor fixture did not reach history',30000,100);
  assert.equal(first.accountWide,true);
  assert.equal(new Set(asked).size,32);
  assert.equal(asked.length,32);
  const initialCost=first.days.reduce((sum,d)=>sum+d.cost,0);
  assert.ok(initialCost>0);
  write(catalog,rates(2));
  const repriced=await until(async()=>{const row=await read();return row&&Math.abs(row.days.reduce((sum,d)=>sum+d.cost,0)-2*initialCost)<1e-12&&row;},'Saved Cursor counts were not repriced');
  assert.equal(asked.length,32,'A new price catalog must not reread the maker’s event pages');
  await resident.stop();
  const kept=new DatabaseSync(path.join(box.home,'agents','usage','insights.sqlite'));
  kept.prepare('UPDATE cursor_account SET scanned=scanned-61000').run();kept.close();
  asked.length=0;failToday=true;
  resident=box.start();view=box.connect(await resident.ready());
  await until(()=>asked.length>=3,'The incomplete today page was not retried',20000,100);
  const after=await read();
  assert.deepEqual(after.days,repriced.days,'An incomplete day cannot replace a complete cached day');
  assert.deepEqual([...new Set(asked)].sort(),[today-day,today]);
});

test('engine ChatGPT history uses read-only OAuth windows, not paid API calls or duplicate channels',async t=>{
  if(boxBinary().typescript)return t.skip('The TypeScript scratch bridge disables engine sign-in reads; Rust reads the isolated maker database directly.');
  const fs=await import('node:fs');
  const {DatabaseSync}=await import('node:sqlite');
  const {computer}=await bench(t);
  const box=await computer('EngineCodexHistory',undefined,isolated);
  const now=Date.now(),signed=now-3600000;
  const oauth={type:'oauth',access:'fixture-only',refresh:'fixture-refresh',expires:now+3600000};
  box.engine.credentials=[{integrationID:'openai',active:true,value:oauth}];
  const folder=path.join(box.home,'data','opencode');fs.mkdirSync(folder,{recursive:true});
  const channel=name=>{
    const db=new DatabaseSync(path.join(folder,name));
    db.exec('CREATE TABLE migration(id TEXT PRIMARY KEY); CREATE TABLE credential(id TEXT PRIMARY KEY,integration_id TEXT,value TEXT,active INTEGER,time_created INTEGER,time_updated INTEGER); CREATE TABLE session_message(id TEXT PRIMARY KEY,session_id TEXT,type TEXT,time_created INTEGER,time_updated INTEGER,data TEXT); CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT)');
    db.prepare('INSERT INTO migration VALUES (?)').run('20260805200742_import_legacy_credentials');
    db.prepare('INSERT INTO credential VALUES (?,?,?,?,?,?)').run('fixture','openai',JSON.stringify(oauth),1,signed,signed);
    return db;
  };
  const data=(at,input,cost)=>JSON.stringify({role:'assistant',time:{completed:at},model:{providerID:'openai',id:'gpt-5.4'},tokens:{input,output:10,cache:{read:5,write:0}},cost});
  const add=(db,id,at,input,cost)=>db.prepare('INSERT INTO session_message VALUES (?,?,?,?,?,?)').run(id,'ses_engine_codex','assistant',at,at,data(at,input,cost));
  let db=channel('opencode.db');
  db.prepare('INSERT INTO message VALUES (?,?,?,?,?)').run('legacy','ses_engine_codex',signed-1000,signed-1000,data(signed-1000,100,0));
  add(db,'shared',now-10000,40,0);
  add(db,'old-key',signed-1000,1000,0);
  add(db,'paid-key',now-10000,1000,2);
  db.close();
  db=channel('opencode-second.db');add(db,'shared',now-10000,40,0);db.close();
  fs.unlinkSync(path.join(box.home,'run','engine.json'));
  const resident=box.start();const call=box.connect(await resident.ready());
  const read=async()=>{const reply=await call('GET','kuvo/agents/api/kuvo/usage?zone=UTC');assert.equal(reply.status,200,reply.text);return reply.json.data.rows.find(row=>row.provider==='openai');};
  const row=await until(async()=>{const row=await read();return row?.days?.reduce((sum,day)=>sum+day.tokens,0)===170&&row;},'The engine’s OAuth usage did not reach the Codex account',20000,100);
  assert.equal(row.service,'codex');
  assert.notEqual(row.estimated,false);
  assert.equal(fs.existsSync(path.join(box.home,'run','engine.json')),false);
  for(const name of ['opencode.db','opencode-second.db']){
    db=new DatabaseSync(path.join(folder,name));db.exec('DELETE FROM credential');db.close();
  }
  await until(async()=>!(await read()),'A signed-out engine’s old ChatGPT usage stayed visible',20000,100);
});

test('Claude file-owned reset grants spend the maker’s named next grant once, with a saved request',{timeout:120000},async t=>{
  if(boxBinary().typescript)return t.skip('The TypeScript Mac scratch bridge does not read Linux file-owned grant fixtures.');
  const http=await import('node:http');
  const {scriptedClaude}=await import('./claude.mjs');
  const org='00000000-0000-4000-8000-000000000001';
  const at=Date.now();
  let outcome='unknown',reads=0,profile='grant-fixture';
  const spent=[];
  const maker=http.createServer((req,res)=>{
    const url=new URL(req.url,'http://127.0.0.1');
    const send=(status,value)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
    assert.equal(req.headers.authorization,'Bearer fixture-claude-grant');
    if(url.pathname==='/claude/oauth/usage'){
      reads++;assert.equal(url.searchParams.get('cedar_ember'),'1');assert.equal(url.searchParams.get('skip_spend'),'1');
      send(200,{cedar_ember:{eligible:true,next_grant_id:'grant-b',grants:[{id:'grant-a',resets_left:1,ends_at:new Date(at+3600000).toISOString(),clears:['five_hour']},{id:'grant-b',resets_left:1,ends_at:new Date(at+7200000).toISOString(),clears:['seven_day']}]}});return;
    }
    if(url.pathname==='/claude/oauth/profile'){send(200,{account:{uuid:profile},organization:{uuid:org}});return;}
    if(url.pathname==='/claude/organizations/'+org+'/reset_rate_limits'){
      let body='';req.on('data',c=>body+=c);req.on('end',()=>{spent.push(JSON.parse(body));send(200,{result:outcome});});return;
    }
    send(404,{});
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const claude=await scriptedClaude();
  t.after(()=>claude.close());
  claude.env.KUVO_TEST_USAGE_BASE='http://127.0.0.1:'+maker.address().port+'/';
  write(path.join(claude.env.CLAUDE_CONFIG_DIR,'.claude.json'),JSON.stringify({oauthAccount:{accountUuid:'grant-fixture',organizationUuid:org}}));
  write(path.join(claude.env.CLAUDE_CONFIG_DIR,'.credentials.json'),JSON.stringify({claudeAiOauth:{accessToken:'fixture-claude-grant',expiresAt:at+3600000,scopes:['user:profile']}}));
  write(path.join(claude.env.HOME,'claude-usage.json'),JSON.stringify({subscription_type:'max',rate_limits_available:true,rate_limits:{seven_day:{utilization:100,resets_at:new Date(at+7200000).toISOString()}}}));
  let listener=await claude.start();
  const base='kuvo/agents/api/kuvo/limits';
  const refreshed=await request(listener,'POST',base+'/refresh',{});
  assert.equal(refreshed.status,200,refreshed.text);
  const row=await until(async()=>{const answer=await request(listener,'GET',base);return answer.json.data.rows.find(r=>r.provider==='claude-code'&&r.extras?.some(e=>e.kind==='resets'&&e.usable));},'Claude grant inventory did not arrive');
  const post=body=>request(listener,'POST',base+'/reset',body);
  const offered=await post({row:row.id});
  assert.equal(offered.status,200,offered.text);assert.equal(offered.json.data.count,2);
  assert.deepEqual(offered.json.data.refills,['seven_day']);assert.equal(spent.length,0);
  const confirm=offered.json.data.confirm;
  const first=await post({row:row.id,confirm});
  assert.equal(first.json.data.outcome,'unknown');assert.equal(spent.length,1);
  assert.equal(spent[0].grant_id,'grant-b');
  assert.equal(reads,1,'Grant inventory is not fetched again for confirmation');
  await claude.stop();listener=await claude.start();
  outcome='already_used';
  const replay=await post({row:row.id,confirm});
  assert.equal(replay.json.data.outcome,'reset');assert.equal(spent.length,2);
  assert.deepEqual(spent[0],spent[1]);
  await post({row:row.id,confirm});assert.equal(spent.length,2);
  const next=await post({row:row.id});assert.equal(next.status,200,next.text);
  profile='other-account';
  const refused=await post({row:row.id,confirm:next.json.data.confirm});
  assert.notEqual(refused.json?.data?.outcome,'reset');assert.equal(spent.length,2);
});

test('a peer remembers a crossing even after its visible notice expires (Rust correction)',{timeout:120000},async t=>{
  if(boxBinary().typescript)return t.skip('Tests the corrected accepted-window handoff.');
  const fs=await import('node:fs');
  const http=await import('node:http');
  const now=Date.now(),reset=Math.floor(now/1000)+7200;
  let studioPercent=89,labPercent=90;
  const maker=http.createServer((req,res)=>{
    const used=req.headers.authorization==='Bearer fixture-studio'?studioPercent:labPercent;
    res.writeHead(200,{'Content-Type':'application/json'});
    res.end(JSON.stringify({plan_type:'plus',rate_limit:{primary_window:{used_percent:used,limit_window_seconds:18000,reset_at:reset}}}));
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer,leases}=await bench(t);
  const env=folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'});
  const studio=await computer('CrossingStudio',undefined,env), lab=await computer('CrossingLab',undefined,env);
  const sign=access=>[{integrationID:'openai',active:true,value:{type:'oauth',access,expires:now+3600000,metadata:{accountID:'same-crossing-account'}}}];
  studio.engine.credentials=sign('fixture-studio');lab.engine.credentials=sign('fixture-lab');
  const base='kuvo/agents/api/kuvo/limits';
  let resident=studio.start(),main=studio.connect(await resident.ready());
  assert.equal((await main('PUT',route,{notifications:{underTenPercent:true,healthyToClose:false,closeToRunningOut:false}})).status,200);
  const read=async call=>(await call('GET',base)).json.data;
  const refresh=async(call,expected)=>{
    assert.equal((await call('POST',base+'/refresh',{})).status,200);
    return until(async()=>{const data=await read(call);return !data.refreshPending&&data.rows.find(r=>r.provider==='openai')?.rings.short?.used===expected&&data;},'The crossing fixture reading did not settle');
  };
  await refresh(main,0.89);studioPercent=91;
  const crossed=await refresh(main,0.91);
  assert.equal(crossed.notices.filter(n=>n.milestone==='underTenPercent').length,1);
  await resident.stop();
  const file=path.join(studio.home,'agents','usage','accepted-limits.json');
  const saved=JSON.parse(fs.readFileSync(file,'utf8'));
  for(const entry of Object.values(saved.entries))for(const notice of entry.notices??[])notice.at=now-660000;
  write(file,JSON.stringify(saved));
  resident=studio.start();main=studio.connect(await resident.ready());
  const setup=await main('GET','kuvo/setup');
  const invite=await main('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});
  assert.equal(invite.status,201,invite.text);
  await until(()=>leases.leaseFor(setup.json.setup),'Studio has no relay lease');
  assert.equal((await lab.join(invite.json.code)).code,0);
  const call=lab.connect(await lab.start().ready());
  await until(async()=>{const value=await read(call);return value.rows.find(r=>r.provider==='openai')?.rings.short?.used===0.91;},'Lab did not receive Studio’s accepted window',30000,250);
  await refresh(call,0.90);
  await until(async()=>{const data=(await call('GET',route)).json.data;return data.settings.notifications?.underTenPercent===true;},'Lab did not receive notification settings');
  labPercent=92;
  const repeated=await refresh(call,0.92);
  assert.equal(repeated.notices.filter(n=>n.milestone==='underTenPercent').length,0);
});

test('a computer without the plan sign-in spends once on the holder, and shares Always Allow decisions',{timeout:120000},async t=>{
  if(boxBinary().typescript)return t.skip('The scratch bridge cannot read the isolated Linux Claude grants.');
  const http=await import('node:http');
  const {claudeEnvironment}=await import('./claude.mjs');
  const {isolatedEnv}=await import('./adapter.mjs');
  const org='00000000-0000-4000-8000-000000000001';
  const at=Date.now();
  const spends=[];
  let outcome='unknown';
  const maker=http.createServer((req,res)=>{
    const url=new URL(req.url,'http://127.0.0.1');
    const send=(status,value)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
    assert.equal(req.headers.authorization,'Bearer fixture-remote-grant');
    if(url.pathname==='/claude/oauth/usage'){
      send(200,{cedar_ember:{eligible:true,next_grant_id:'grant-a',grants:[{id:'grant-a',resets_left:3,ends_at:new Date(at+7200000).toISOString(),clears:['seven_day']}]}});return;
    }
    if(url.pathname==='/claude/oauth/profile'){send(200,{account:{uuid:'remote-grant-fixture'},organization:{uuid:org}});return;}
    if(url.pathname==='/claude/organizations/'+org+'/reset_rate_limits'){
      let body='';req.on('data',c=>body+=c);req.on('end',()=>{spends.push(JSON.parse(body));send(200,{result:outcome});});return;
    }
    send(404,{});
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer,leases}=await bench(t);
  const holder=await computer('ResetHolder',undefined,folder=>({...claudeEnvironment(boxBinary(),path.join(folder,'box'),isolatedEnv(folder),{turns:true}),...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const asker=await computer('ResetAsker',undefined,isolated);
  write(path.join(holder.env.CLAUDE_CONFIG_DIR,'.claude.json'),JSON.stringify({oauthAccount:{accountUuid:'remote-grant-fixture',organizationUuid:org}}));
  write(path.join(holder.env.CLAUDE_CONFIG_DIR,'.credentials.json'),JSON.stringify({claudeAiOauth:{accessToken:'fixture-remote-grant',expiresAt:at+3600000,scopes:['user:profile']}}));
  write(path.join(holder.env.HOME,'claude-usage.json'),JSON.stringify({subscription_type:'max',rate_limits_available:true,rate_limits:{seven_day:{utilization:100,resets_at:new Date(at+7200000).toISOString()}}}));
  const here=holder.connect(await holder.start().ready());
  const base='kuvo/agents/api/kuvo/limits';
  await here('POST',base+'/refresh',{});
  const row=await until(async()=>{const data=(await here('GET',base)).json.data;return data.rows.find(r=>r.provider==='claude-code'&&r.extras?.some(e=>e.kind==='resets'&&e.usable));},'The holder did not read its own grants');
  const setup=await here('GET','kuvo/setup');
  const invite=await here('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});
  assert.equal(invite.status,201,invite.text);
  await until(()=>leases.leaseFor(setup.json.setup),'Holder did not claim a relay lease');
  assert.equal((await asker.join(invite.json.code)).code,0);
  let resident=asker.start(),call=asker.connect(await resident.ready());
  const remote=async()=>{const r=(await call('GET',base)).json.data.rows.find(r=>r.id===row.id);return r?.extras?.find(e=>e.kind==='resets');};
  await until(async()=>{const extra=await remote();return extra?.on?.name==='ResetHolder'&&extra;},'Remote grant account did not arrive',30000,250);
  const plan=await call('POST','kuvo/agent-tools',{name:'plan_limits',arguments:{}});
  assert.match(plan.json.text,/signed in on ResetHolder/);
  assert.match(plan.json.text,/used on ResetHolder, which has its sign-in/);
  const post=body=>call('POST',base+'/reset',body);
  const offered=await post({row:row.id});
  assert.equal(offered.status,200,offered.text);assert.equal(offered.json.data.on.name,'ResetHolder');
  assert.notEqual(offered.json.data.on.here,true);assert.equal(spends.length,0);
  const confirm=offered.json.data.confirm;
  const pending=await post({row:row.id,confirm});
  assert.equal(pending.json.data.outcome,'unknown');assert.equal(spends.length,1);
  await resident.stop();resident=asker.start();call=asker.connect(await resident.ready());
  outcome='already_used';
  const replay=await post({row:row.id,confirm});
  assert.equal(replay.json.data.outcome,'reset');assert.equal(spends.length,2);assert.deepEqual(spends[0],spends[1]);
  const done=await post({row:row.id,confirm});assert.equal(done.json.data.outcome,'reset');assert.equal(spends.length,2);
  const always=await call('PUT',base+'/reset/always',{row:row.id,always:true});assert.equal(always.status,200,always.text);
  await until(async()=>{const r=(await here('GET',base)).json.data.rows.find(r=>r.id===row.id);return r?.extras?.some(e=>e.kind==='resets'&&e.agents===true);},'Always Allow decision did not reach its holder');
  const off=await here('PUT',base+'/reset/always',{row:row.id,always:false});assert.equal(off.status,200,off.text);
  await until(async()=>{const extra=await remote();return extra&&extra.agents!==true;},'Turning Always Allow off did not reach the other computer');
});

test('a copied billed engine message uses the first channel and falls back when it is removed',async t=>{
  const fs=await import('node:fs');
  const {DatabaseSync}=await import('node:sqlite');
  const {computer}=await bench(t);
  const box=await computer('BilledChannels',undefined,isolated);
  const folder=path.join(box.home,'data','opencode');fs.mkdirSync(folder,{recursive:true});
  const now=Date.now()-10000;
  const add=(name,cost,tokens)=>{
    const file=path.join(folder,name),db=new DatabaseSync(file);
    db.exec('CREATE TABLE session_message(id TEXT PRIMARY KEY,session_id TEXT,type TEXT,time_created INTEGER,time_updated INTEGER,data TEXT)');
    const insert=db.prepare('INSERT INTO session_message VALUES (?,?,?,?,?,?)');
    insert.run('same','ses_billed_channels','assistant',now,now,JSON.stringify({time:{completed:now},model:{providerID:'opencode-go',id:'glm-5'},tokens:{total:tokens},cost}));
    insert.run('not-billed','ses_billed_channels','assistant',now,now,JSON.stringify({time:{completed:now},model:{providerID:'opencode-go',id:'glm-5'},tokens:{total:999},cost:'1.25'}));
    insert.run('unfinished','ses_billed_channels','compaction',now,now,JSON.stringify({status:'running',model:{providerID:'opencode-go',id:'glm-5'},tokens:{total:999},cost:1.25}));
    db.close();return file;
  };
  const first=add('opencode-a.db',0.25,100),second=add('opencode-b.db',0.5,200);
  const call=box.connect(await box.start().ready());
  const base='kuvo/agents/api/kuvo/usage';
  const read=async()=>{const result=await call('GET',base+'?zone=UTC');assert.equal(result.status,200,result.text);return result.json.data.rows.find(r=>r.provider==='opencode-go');};
  await call('POST',base+'/refresh?zone=UTC',{});
  const initial=await until(async()=>{const row=await read();return row?.days?.[0]?.tokens===100&&row;},'The first maker channel should own a copied billed message');
  assert.equal(initial.days[0].cost,0.25);
  fs.unlinkSync(first);
  if(boxBinary().typescript)await call('POST',base+'/refresh?zone=UTC',{});
  const retained=await until(async()=>{const row=await read();return row?.days?.[0]?.tokens===200&&row;},'The remaining channel did not replace the removed one');
  assert.equal(retained.days[0].cost,0.5);
  fs.unlinkSync(second);
  if(boxBinary().typescript)await call('POST',base+'/refresh?zone=UTC',{});
  await until(async()=>!(await read()),'Removing all channels left billed history');
});

test('Antigravity never resends a refused bearer or refresh token after restart',async t=>{
  if(boxBinary().typescript)return t.skip('Scratch bridge disables provider requests.');
  const fs=await import('node:fs');
  const http=await import('node:http');
  const {computer}=await bench(t);
  const at=Date.now(),calls=[];
  let renewal=500,returned='fixture-antigravity-bad';
  const maker=http.createServer((req,res)=>{
    const finish=(status,body)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(body));};
    if(req.url==='/antigravity/token'){
      let raw='';req.on('data',c=>raw+=c);req.on('end',()=>{const form=new URLSearchParams(raw);assert.equal(form.get('refresh_token'),'fixture-antigravity-refresh');assert.ok(form.get('client_id'));assert.ok(form.get('client_secret'));calls.push('refresh');finish(renewal,{access_token:returned,expires_in:3600});});return;
    }
    calls.push(req.headers.authorization??'missing');finish(401,{});
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const box=await computer('AntigravityRefusals',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const auth=path.join(box.home,'agents','antigravity','profile','antigravity-acp','acp_token.json');
  const credential=(access)=>JSON.stringify({access_token:access,refresh_token:'fixture-antigravity-refresh',expires_at:Math.floor(at/1000)+3600});
  write(auth,credential('fixture-antigravity-bad'));
  let resident=box.start(),call=box.connect(await resident.ready());
  const base='kuvo/agents/api/kuvo/limits';
  const refresh=async()=>{const answer=await call('POST',base+'/refresh',{});assert.equal(answer.status,200,answer.text);await until(async()=>!(await call('GET',base)).json.data.refreshPending,'The quota read did not settle');};
  await refresh();
  assert.ok(calls.includes('Bearer fixture-antigravity-bad'));assert.ok(calls.includes('refresh'));
  const journal=path.join(box.home,'agents','usage','antigravity','default','refusals.json');
  const kept=JSON.parse(fs.readFileSync(journal,'utf8'));assert.equal(kept.access.length,1);assert.match(kept.access[0],/^[a-f0-9]{16}$/);
  assert.ok(!fs.readFileSync(journal,'utf8').includes('fixture-antigravity'));
  await resident.stop();calls.length=0;renewal=200;
  resident=box.start();call=box.connect(await resident.ready());await refresh();
  assert.ok(calls.includes('refresh'));assert.ok(calls.every(c=>c==='refresh'),'A refresh reply returning the refused bearer is not sent to a quota host');
  await resident.stop();calls.length=0;renewal=400;
  resident=box.start();call=box.connect(await resident.ready());await refresh();
  assert.ok(calls.includes('refresh'));assert.ok(JSON.parse(fs.readFileSync(journal,'utf8')).refresh.length);
  await resident.stop();calls.length=0;write(auth,credential(undefined));
  resident=box.start();call=box.connect(await resident.ready());await refresh();
  assert.deepEqual(calls,[],'A refused refresh token stays refused even after access metadata changes');
});

test('Antigravity keeps another host’s usable windows while one quota call is held',async t=>{
  if(boxBinary().typescript)return t.skip('Scratch bridge disables provider requests.');
  const http=await import('node:http');
  const at=Date.now(),calls=[];
  const maker=http.createServer((req,res)=>{
    calls.push(req.url);assert.equal(req.headers.authorization,'Bearer fixture-ag-held');
    if(req.url.includes('/main/')||req.url.endsWith('loadCodeAssist')){res.writeHead(429,{'Retry-After':'3600','Content-Type':'application/json'});res.end('{}');return;}
    res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify({groups:[{displayName:'Gemini',buckets:[{bucketId:'gemini-5h',remainingFraction:0.2,resetTime:new Date(at+7200000).toISOString()},{bucketId:'gemini-weekly',remainingFraction:0.7,resetTime:new Date(at+7*86400000).toISOString()}]}]}));
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('AntigravityHostHold',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  write(path.join(box.home,'agents','antigravity','profile','antigravity-acp','acp_token.json'),JSON.stringify({access_token:'fixture-ag-held',expires_at:Math.floor(at/1000)+3600}));
  const call=box.connect(await box.start().ready());const base='kuvo/agents/api/kuvo/limits';
  const refresh=async()=>{assert.equal((await call('POST',base+'/refresh',{})).status,200);return until(async()=>{const data=(await call('GET',base)).json.data;return !data.refreshPending&&data.rows.find(r=>r.provider==='antigravity'&&r.rings.short?.used===0.8);},'The usable host’s quota did not arrive');};
  await refresh();const first=calls.slice();assert.ok(first.some(url=>url.includes('/main/')));assert.ok(first.some(url=>url.includes('/daily/')));
  await refresh();assert.equal(calls.filter(url=>url.includes('/main/')).length,first.filter(url=>url.includes('/main/')).length,'A held host is not retried by the other host’s successful read');
});

test('Antigravity reads only its running profile’s owned loopback TLS server without a bearer token',async t=>{
  if(boxBinary().typescript)return t.skip('The TypeScript scratch bridge disables provider requests.');
  if(process.platform==='win32')return t.skip('The synthetic /proc fixture is Unix-only; the native Windows case exercises actual process ownership.');
  const fs=await import('node:fs');
  const https=await import('node:https');
  const http=await import('node:http');
  const {spawnSync}=await import('node:child_process');
  const {computer}=await bench(t);
  const at=Date.now(),methods=[],cloudCalls=[];
  let localStatus=200,localCsrf='fixture-csrf';
  const cloud=http.createServer((req,res)=>{
    cloudCalls.push(req.url);assert.equal(req.headers.authorization,'Bearer fixture-cloud-token');
    const body=req.url.includes('loadCodeAssist')?{paidTier:{id:'g1-pro-tier'}}:{groups:[{buckets:['gemini-5h','gemini-weekly','3p-5h','3p-weekly'].map(bucketId=>({bucketId,remainingFraction:0.5,resetTime:new Date(at+7200000).toISOString()}))}]};
    res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify(body));
  });
  await new Promise(resolve=>cloud.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{cloud.close(resolve);cloud.closeAllConnections();}));
  // Temporary self-signed fixture, never an installed trust root or a real key.
  const box=await computer('AntigravityOwnedServer',undefined,folder=>({...isolated(folder),KUVO_TEST_AG_PROC_ROOT:path.join(folder,'account','fixture-proc'),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+cloud.address().port+'/'}));
  const key=path.join(box.env.HOME,'fixture-key.pem'),cert=path.join(box.env.HOME,'fixture-cert.pem');
  const made=spawnSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-subj','/CN=fixture.invalid','-days','1','-keyout',key,'-out',cert],{stdio:'ignore'});
  assert.equal(made.status,0,'The fixture certificate could not be generated');
  const server=https.createServer({key:fs.readFileSync(key),cert:fs.readFileSync(cert)},(req,res)=>{
    const method=req.url.split('/').at(-1);methods.push(method);
    assert.equal(req.headers.authorization,undefined);
    assert.equal(req.headers['x-codeium-csrf-token']??'',localCsrf);
    if(localStatus!==200){res.writeHead(localStatus,{'Content-Type':'application/json'});res.end('{}');return;}
    const answer=method==='GetUserStatus'?{userStatus:{userTier:{name:'Pro'}}}:{groups:[{buckets:['gemini-5h','gemini-weekly','3p-5h','3p-weekly'].map(bucketId=>({bucketId,remainingFraction:0.2,resetTime:new Date(at+7200000).toISOString()}))}]};
    res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify(answer));
  });
  await new Promise(resolve=>server.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{server.close(resolve);server.closeAllConnections();}));
  const profile=path.join(box.home,'agents','antigravity','profile');
  write(path.join(profile,'antigravity-acp','kuvo-sign-in.json'),JSON.stringify({id:'fixture-local-sign-in'}));
  const proc=box.env.KUVO_TEST_AG_PROC_ROOT;
  const add=(pid,own,csrf,inode,port)=>{
    const folder=path.join(proc,String(pid));
    write(path.join(folder,'cmdline'),'/bin/language_server\0--csrf_token\0'+csrf+'\0');
    write(path.join(folder,'environ'),'GEMINI_HOME='+own+'\0');
    fs.mkdirSync(path.join(folder,'fd'),{recursive:true});
    fs.symlinkSync('socket:['+inode+']',path.join(folder,'fd','5'));
    const line=(p,i)=>'0: 0100007F:'+p.toString(16).padStart(4,'0')+' 00000000:0000 0A 00000000:00000000 00:00000000 00000000 1000 0 '+i+'\n';
    write(path.join(folder,'net','tcp'),'sl local_address rem_address st tx_queue tr tm->when retrnsmt uid timeout inode\n'+line(1,999)+line(port,inode));
  };
  add(101,profile+'-not-ours','wrong-csrf',101,server.address().port);
  add(102,profile,'fixture-csrf',102,server.address().port);
  let resident=box.start(),call=box.connect(await resident.ready());const base='kuvo/agents/api/kuvo/limits';
  const refresh=async check=>{
    assert.equal((await call('POST',base+'/refresh',{})).status,200);
    return until(async()=>{const data=(await call('GET',base)).json.data;return !data.refreshPending&&data.rows.find(r=>r.provider==='antigravity'&&check(r));},'Antigravity local/cloud fixture did not settle');
  };
  const row=await refresh(r=>r.rings.short?.used===0.8);
  assert.equal(row.plan,'Pro');assert.ok(methods.includes('RetrieveUserQuotaSummary'));assert.ok(methods.includes('GetUserStatus'));
  assert.equal(cloudCalls.length,0);
  fs.rmSync(path.join(proc,'102'),{recursive:true});
  const stopped=await refresh(r=>r.problem?.kind==='failed');
  assert.equal(stopped.problem.message,'Antigravity says its usage while it runs on this computer. Start a conversation with it to see it.');
  assert.equal(cloudCalls.length,0);
  add(102,profile,'fixture-csrf',102,server.address().port);
  write(path.join(profile,'antigravity-acp','acp_token.json'),JSON.stringify({access_token:'fixture-cloud-token',expires_at:Math.floor(at/1000)+3600}));
  localStatus=403;const before=methods.length;
  const fallback=await refresh(r=>r.rings.short?.used===0.5);assert.equal(fallback.problem,undefined);
  assert.equal(methods.length,before+1,'Refused local CSRF is not sent to another endpoint');
  assert.ok(cloudCalls.length>0);
  const rejected=methods.length;
  await resident.stop();resident=box.start();call=box.connect(await resident.ready());
  await refresh(r=>r.rings.short?.used===0.5);
  assert.equal(methods.length,rejected,'The refused CSRF survives restart');
  // Authless agy has no credential to blacklist. It is retried when the
  // language server answers again, with no invented sign-in or cloud call.
  fs.rmSync(path.join(profile,'antigravity-acp','acp_token.json'));
  write(path.join(proc,'102','cmdline'),'/bin/agy\0');
  localCsrf='';let authless=methods.length,cloudBefore=cloudCalls.length;
  await refresh(r=>r.problem?.kind==='failed');assert.ok(methods.length>authless);
  localStatus=200;authless=methods.length;
  await refresh(r=>r.rings.short?.used===0.8&&r.problem===undefined);
  assert.ok(methods.length>authless);assert.equal(cloudCalls.length,cloudBefore);
});

test('the engine ChatGPT reset credit uses the saved OAuth identity without starting the engine',async t=>{
  if(boxBinary().typescript)return t.skip('The TS scratch bridge disables engine sign-in calls.');
  const fs=await import('node:fs');
  const http=await import('node:http');
  const {DatabaseSync}=await import('node:sqlite');
  const {computer}=await bench(t);
  const at=Date.now(),reset=Math.floor(at/1000)+7200;let outcome='unknown',delay=0,creditList=[{id:'late',status:'available',expires_at:reset+7200},{id:'soon',status:'available',expires_at:reset+3600}];const spent=[];
  const maker=http.createServer((req,res)=>{
    const send=(status,value)=>{res.writeHead(status,{'Content-Type':'application/json'});res.end(JSON.stringify(value));};
    assert.equal(req.headers.authorization,'Bearer fixture-engine-reset');
    if(req.url==='/chatgpt'){send(200,{plan_type:'plus',rate_limit:{primary_window:{used_percent:100,limit_window_seconds:18000,reset_at:reset}},rate_limit_reset_credits:{available_count:creditList.length}});return;}
    assert.equal(req.headers['chatgpt-account-id'],'fixture-engine-reset-account');
    assert.equal(req.headers['openai-beta'],'codex-1');
    if(req.url==='/chatgpt/resets'){send(200,{available_count:creditList.length,credits:creditList});return;}
    if(req.url==='/chatgpt/resets/consume'){
      let body='';req.on('data',c=>body+=c);req.on('end',()=>{const used=JSON.parse(body),saved=JSON.parse(fs.readFileSync(path.join(box.home,'agents','limits-resets.json'),'utf8'));assert.ok(saved.spends.some(s=>s.request===used.redeem_request_id&&s.target===used.credit_id&&s.state==='sending'));spent.push(used);setTimeout(()=>send(200,{code:outcome}),delay);});return;
    }
    send(404,{});
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const box=await computer('EngineReset',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const folder=path.join(box.home,'data','opencode');fs.mkdirSync(folder,{recursive:true});const database=path.join(folder,'opencode.db');
  const credential=accountID=>JSON.stringify({type:'oauth',access:'fixture-engine-reset',expires:at+3600000,metadata:{accountID}});
  let db=new DatabaseSync(database);db.exec('CREATE TABLE credential(id TEXT,integration_id TEXT,value TEXT,active INTEGER,time_updated INTEGER)');db.prepare('INSERT INTO credential VALUES(?,?,?,?,?)').run('one','openai',credential('fixture-engine-reset-account'),1,at);db.close();
  const engineFile=path.join(box.home,'run','engine.json');fs.unlinkSync(engineFile);
  let resident=box.start(),call=box.connect(await resident.ready());const base='kuvo/agents/api/kuvo/limits';
  assert.equal((await call('POST',base+'/refresh',{})).status,200);
  const row=await until(async()=>{const data=(await call('GET',base)).json.data;return !data.refreshPending&&data.rows.find(r=>r.provider==='openai'&&r.extras?.some(e=>e.kind==='resets'&&e.usable));},'Engine account did not offer its credits');
  const offer=await call('POST',base+'/reset',{row:row.id});assert.equal(offer.status,200,offer.text);assert.equal(offer.json.data.count,2);assert.equal(spent.length,0);
  const confirm=offer.json.data.confirm;
  const post=()=>call('POST',base+'/reset',{row:row.id,confirm});
  assert.equal((await post()).json.data.outcome,'unknown');assert.equal(spent.length,1);assert.equal(spent[0].credit_id,'soon');
  await resident.stop();resident=box.start();call=box.connect(await resident.ready());outcome='already_redeemed';
  assert.equal((await post()).json.data.outcome,'reset');assert.equal(spent.length,2);assert.deepEqual(spent[0],spent[1]);
  await post();assert.equal(spent.length,2);
  creditList=creditList.filter(c=>c.id==='late');outcome='reset';delay=200;
  await call('POST',base+'/refresh',{});
  await until(async()=>{const data=(await call('GET',base)).json.data;return !data.refreshPending&&data.rows.find(r=>r.id===row.id)?.extras?.find(e=>e.kind==='resets')?.count===1;},'Remaining credit was not refreshed');
  const offers=await Promise.all([1,2].map(()=>call('POST',base+'/reset',{row:row.id})));
  const both=await Promise.all(offers.map(o=>call('POST',base+'/reset',{row:row.id,confirm:o.json.data.confirm})));
  assert.ok(both.every(r=>r.json.data.outcome==='reset'));assert.equal(spent.length,3,'Overlapping confirmations share one newly spent credit');assert.equal(spent[2].credit_id,'late');assert.notEqual(spent[2].redeem_request_id,spent[0].redeem_request_id);
  assert.equal(fs.existsSync(engineFile),false);assert.equal(box.engine.asked.filter(r=>r.path==='/api/credential').length,0);
});

test('an older unsettled reset is settled before a later confirmation spends another credit',async t=>{
  if(boxBinary().typescript)return t.skip('Rust native fixture.');
  const fs=await import('node:fs');
  const codex=await scriptedCodex();t.after(()=>codex.close());
  const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:'fixture',chatgpt_user_id:'fixture-user'}})).toString('base64url'),'fixture'].join('.');
  write(path.join(codex.root,'.codex','auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:'fixture'}}));
  const reset=Math.floor(Date.now()/1000)+7200;
  const inventory=credits=>JSON.stringify({accountId:'fixture',rateLimits:{planType:'plus',primary:{usedPercent:100,windowDurationMins:300,resetsAt:reset}},rateLimitResetCredits:{availableCount:credits.length,credits}});
  const soon={id:'soon',status:'available',expiresAt:reset+3600},late={id:'late',status:'available',expiresAt:reset+7200};
  write(path.join(codex.root,'codex-limits.json'),inventory([late,soon]));
  write(path.join(codex.root,'codex-reset-answer.json'),JSON.stringify({outcome:'unknown'}));
  let listener=await codex.start();const base='kuvo/agents/api/kuvo/limits';
  const read=async()=>{assert.equal((await request(listener,'POST',base+'/refresh',{})).status,200);return until(async()=>{const result=await request(listener,'GET',base);return !result.json.data.refreshPending&&result.json.data.rows.find(r=>r.provider==='codex-cli'&&r.extras?.some(e=>e.kind==='resets'&&e.usable));},'Reset account did not settle');};
  const row=await read();const post=body=>request(listener,'POST',base+'/reset',body);
  const offer=await post({row:row.id});assert.equal((await post({row:row.id,confirm:offer.json.data.confirm})).json.data.outcome,'unknown');
  const first=codex.sent('account/rateLimitResetCredit/consume');assert.equal(first.length,1);
  await codex.stop();
  const file=path.join(codex.home,'agents','limits-resets.json'),kept=JSON.parse(fs.readFileSync(file,'utf8'));
  kept.spends[0].at=Date.now()-11*60000;write(file,JSON.stringify(kept));
  write(path.join(codex.root,'codex-limits.json'),inventory([late]));write(path.join(codex.root,'codex-reset-answer.json'),JSON.stringify({outcome:'reset'}));
  listener=await codex.start();await read();
  const next=await post({row:row.id});assert.equal(next.status,200,next.text);
  const done=await post({row:row.id,confirm:next.json.data.confirm});assert.equal(done.json.data.outcome,'reset');
  const spends=codex.sent('account/rateLimitResetCredit/consume');assert.equal(spends.length,3);assert.deepEqual(spends[0].params,spends[1].params);assert.equal(spends[2].params.creditId,'late');assert.notEqual(spends[2].params.idempotencyKey,spends[0].params.idempotencyKey);
  await post({row:row.id,confirm:next.json.data.confirm});assert.equal(codex.sent('account/rateLimitResetCredit/consume').length,3);
});

test('Codex history prefers a live session’s tier to its copied archive and reads each configured home',async t=>{
  const fs=await import('node:fs');const {computer}=await bench(t);
  const box=await computer('CodexRoots',undefined,folder=>({...isolated(folder),CODEX_HOME:[path.join(folder,'codex-one'),path.join(folder,'codex-two')].join(',')}));
  const [one,two]=box.env.CODEX_HOME.split(',');const at=Date.now()-60000;
  const event=(when,priority)=>[JSON.stringify({type:'session_meta',timestamp:new Date(when).toISOString(),payload:{id:'fixture-'+when}}),JSON.stringify({type:'turn_context',payload:{model:'gpt-5.5'}}),...(priority?[JSON.stringify({type:'event_msg',payload:{type:'thread_settings_applied',service_tier:'priority'}})]:[]),JSON.stringify({type:'event_msg',timestamp:new Date(when).toISOString(),payload:{type:'token_count',info:{last_token_usage:{input_tokens:400,cached_input_tokens:0,output_tokens:100,total_tokens:500}}}})].join('\n')+'\n';
  write(path.join(one,'sessions','live.jsonl'),event(at,true));
  write(path.join(one,'archived_sessions','copy.jsonl'),event(at,false));
  // The second named home has neither sessions nor archived_sessions: the home itself counts.
  write(path.join(two,'rollout.jsonl'),event(at+1000,false));
  if(process.platform!=='win32'){
    write(path.join(box.home,'agents','accounts.json'),JSON.stringify([{formatVersion:1},{kind:'codex',id:'linked',label:'Linked'}]));
    const linked=path.join(box.home,'agents','codex','accounts','linked');fs.mkdirSync(linked,{recursive:true});fs.symlinkSync(path.join(one,'sessions'),path.join(linked,'sessions'));
  }
  const call=box.connect(await box.start().ready());const base='kuvo/agents/api/kuvo/usage';
  assert.equal((await call('POST',base+'/refresh?zone=UTC',{})).status,200);
  const result=await until(async()=>{const data=(await call('GET',base+'?zone=UTC')).json.data;const row=data.rows.find(r=>r.provider==='codex-cli');return row?.days?.reduce((n,d)=>n+d.tokens,0)===1000&&data;},'The two configured homes did not reach usage');
  const row=result.rows.find(r=>r.provider==='codex-cli');const baseCost=(400*5)/1e6+(100*30)/1e6;assert.equal(row.days.reduce((sum,d)=>sum+d.cost,0),baseCost*2.5+baseCost,'The live session’s priority tier must win over its archive, with identical arithmetic');
  assert.ok(!result.rows.find(r=>r.provider==='codex-cli@linked')?.days?.some(d=>d.tokens>0));
});

test('native Windows finds only the listening language server of its own Antigravity ACP profile',async t=>{
  if(boxBinary().typescript||process.platform!=='win32')return t.skip('Native Windows process and TCP ownership fixture.');
  const fs=await import('node:fs');const {spawn}=await import('node:child_process');const {computer}=await bench(t);
  const box=await computer('AntigravityWindowsProfile',undefined,folder=>({...isolated(folder),KUVO_TEST_AG_WINDOWS_ROOT:path.join(folder,'account','fixture agy windows'),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:1/'}));
  const root=box.env.KUVO_TEST_AG_WINDOWS_ROOT;fs.mkdirSync(root,{recursive:true});
  const profile=path.join(box.home,'agents','antigravity','profile');const wrongProfile=profile+'-not-ours';
  fs.mkdirSync(wrongProfile,{recursive:true});
  write(path.join(profile,'antigravity-acp','kuvo-sign-in.json'),JSON.stringify({id:'fixture-windows-local-sign-in'}));
  const agy=path.join(root,'agy.exe'),languageServer=path.join(root,'language_server_fixture.exe');
  fs.copyFileSync(process.execPath,agy);fs.copyFileSync(process.execPath,languageServer);
  const script=path.join(root,'server.mjs'),parentScript=path.join(root,'parent.mjs');
  write(script,`import http from 'node:http';import fs from 'node:fs';
const [mode,log,ready]=process.argv.slice(2);const at=Date.now();
const server=http.createServer((req,res)=>{const method=req.url.split('/').at(-1);fs.appendFileSync(log,JSON.stringify({method,authorization:req.headers.authorization,csrf:req.headers['x-codeium-csrf-token']})+'\\n');
const answer=method==='GetUserStatus'?{userStatus:{userTier:{name:'Pro'}}}:{groups:[{buckets:['gemini-5h','gemini-weekly','3p-5h','3p-weekly'].map(bucketId=>({bucketId,remainingFraction:mode==='own'?0.2:0.99,resetTime:new Date(at+7200000).toISOString()}))}]};
res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify(answer));});
server.listen(0,'127.0.0.1',()=>fs.writeFileSync(ready,String(server.address().port)));
process.on('message',m=>{if(m==='stop'){server.closeAllConnections();server.close(()=>process.exit(0));}});`);
  write(parentScript,`import {spawn} from 'node:child_process';
const [exe,script,log,ready]=process.argv.slice(2);
const child=spawn(exe,[script,'own',log,ready,'--csrf_token','fixture-win-csrf'],{stdio:'ignore',env:process.env});
const stop=()=>{if(child.exitCode===null)child.kill();};process.on('message',m=>{if(m==='stop')stop();});process.on('disconnect',stop);process.on('exit',stop);child.on('exit',()=>process.exit(0));`);
  const wrongLog=path.join(root,'wrong.jsonl'),wrongReady=path.join(root,'wrong.ready');const ownLog=path.join(root,'own.jsonl'),ownReady=path.join(root,'own.ready');
  const children=[];
  const start=(exe,args)=>{const child=spawn(exe,args,{cwd:root,env:box.env,stdio:['ignore','ignore','ignore','ipc']});const exited=new Promise(resolve=>child.once('close',resolve));child.on('error',()=>{});children.push({child,exited});return child;};
  try {
  start(agy,[script,'wrong',wrongLog,wrongReady,'--gemini-dir',wrongProfile]);
  await until(()=>fs.existsSync(wrongReady),'Wrong profile server did not start');
  start(agy,[parentScript,languageServer,script,ownLog,ownReady,'--gemini-dir',profile]);
  await until(()=>fs.existsSync(ownReady),'Own profile language server did not start');
  const call=box.connect(await box.start().ready()),base='kuvo/agents/api/kuvo/limits';
  assert.equal((await call('POST',base+'/refresh',{})).status,200);
  const row=await until(async()=>{const data=(await call('GET',base)).json.data;return !data.refreshPending&&data.rows.find(r=>r.provider==='antigravity'&&r.rings.short?.used===0.8);},'Native Windows did not read its own ACP child server');
  assert.equal(row.plan,'Pro');assert.equal(fs.existsSync(wrongLog),false,'Another profile was contacted');
  const methods=fs.readFileSync(ownLog,'utf8').trim().split('\n').map(JSON.parse);assert.ok(methods.some(m=>m.method==='RetrieveUserQuotaSummary'));assert.ok(methods.some(m=>m.method==='GetUserStatus'));
  assert.ok(methods.every(m=>m.authorization===undefined&&m.csrf==='fixture-win-csrf'));
  } finally {
    for(const {child,exited} of children.reverse()){
      if(child.exitCode===null){if(child.connected)child.send('stop');else child.kill();}
      await exited;
    }
  }
});


test('Antigravity retains a missing reset-less pool only for its first partial reading, across restart',async t=>{
  if(boxBinary().typescript)return t.skip('Scratch bridge disables provider requests.');
  const http=await import('node:http');
  const {computer}=await bench(t);let partial=false;
  const at=Date.now();
  const maker=http.createServer((req,res)=>{
    assert.equal(req.headers.authorization,'Bearer fixture-ag-partial');
    const buckets=partial?[{bucketId:'gemini-5h',remainingFraction:0.7,resetTime:new Date(at+7200000).toISOString()}]:[
      {bucketId:'gemini-5h',remainingFraction:0.8,resetTime:new Date(at+7200000).toISOString()},
      {bucketId:'gemini-weekly',remainingFraction:0.4},
      {bucketId:'3p-5h',remainingFraction:0.6,resetTime:new Date(at+7200000).toISOString()},
      {bucketId:'3p-weekly',remainingFraction:0.9,resetTime:new Date(at+7*86400000).toISOString()}
    ];
    const body=req.url.endsWith('loadCodeAssist')?{paidTier:{id:'g1-pro-tier'}}:{groups:[{displayName:'Fixture pools',buckets}]};
    res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify(body));
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const box=await computer('AntigravityPartial',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  write(path.join(box.home,'agents','antigravity','profile','antigravity-acp','acp_token.json'),JSON.stringify({access_token:'fixture-ag-partial',expires_at:Math.floor(at/1000)+3600}));
  let resident=box.start(),call=box.connect(await resident.ready());const base='kuvo/agents/api/kuvo/limits';
  const refresh=async()=>{assert.equal((await call('POST',base+'/refresh',{})).status,200);return until(async()=>{const data=(await call('GET',base)).json.data;return !data.refreshPending&&data.rows.find(r=>r.provider==='antigravity'&&r.windows?.length);},'The partial quota reading did not settle');};
  const first=await refresh();assert.equal(first.windows.length,4);assert.equal(first.plan,'Pro');
  partial=true;
  const next=await refresh();assert.ok(next.windows.some(w=>w.id==='gemini-weekly'));assert.equal(next.windows.length,4);
  await resident.stop();resident=box.start();call=box.connect(await resident.ready());
  const last=await refresh();assert.deepEqual(last.windows.map(w=>w.id).sort(),['3p-5h','3p-weekly','gemini-5h']);assert.equal(last.plan,'Pro');
});

test('Antigravity’s menu takes both rings from the same tightest quota pool',async t=>{
  if(boxBinary().typescript)return t.skip('Scratch bridge disables provider requests.');
  const http=await import('node:http');const {computer}=await bench(t);let second=false;
  const at=Date.now();const maker=http.createServer((req,res)=>{
    const buckets=[['gemini-5h',0.8],['gemini-weekly',0.09],['3p-5h',0.2],['3p-weekly',second?0.04:0.3]].map(([bucketId,remainingFraction])=>({bucketId,remainingFraction,resetTime:new Date(at+7200000).toISOString()}));
    res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify(req.url.endsWith('loadCodeAssist')?{paidTier:{id:'g1-pro-tier'}}:{groups:[{displayName:'Fixture pools',buckets}]}));
  });await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const box=await computer('AntigravityPool',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  write(path.join(box.home,'agents','antigravity','profile','antigravity-acp','acp_token.json'),JSON.stringify({access_token:'fixture-ag-pool',expires_at:Math.floor(at/1000)+3600}));
  const call=box.connect(await box.start().ready()),base='kuvo/agents/api/kuvo/limits';
  const refresh=async()=>{assert.equal((await call('POST',base+'/refresh',{})).status,200);return until(async()=>{const data=(await call('GET',base)).json.data;return !data.refreshPending&&data.rows.find(r=>r.provider==='antigravity'&&r.windows?.length===4);},'Quota pools did not reach the rail');};
  const first=await refresh();assert.equal(first.rings.short.window,'gemini-5h');assert.equal(first.rings.long.window,'gemini-weekly');
  let menu=(await call('GET','kuvo/box/at-work')).json.rings.find(r=>r.service==='antigravity');assert.equal(menu.short.used,first.rings.short.used);assert.equal(menu.long.used,first.rings.long.used);
  second=true;const next=await refresh();assert.equal(next.rings.short.window,'3p-5h');assert.equal(next.rings.long.window,'3p-weekly');
  menu=(await call('GET','kuvo/box/at-work')).json.rings.find(r=>r.service==='antigravity');assert.equal(menu.short.used,next.rings.short.used);assert.equal(menu.long.used,next.rings.long.used);
});


test('an upgrade retains the old accepted reading only for the still-signed-in account',async t=>{
  if(boxBinary().typescript)return t.skip('The old box owns this cache; the upgrade path is Rust-only.');
  const crypto=await import('node:crypto');
  const codex=await scriptedCodex();t.after(()=>codex.close());
  const sign=(account)=>["e30",Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:account,chatgpt_user_id:'fixture-user'}})).toString('base64url'),"fixture"].join('.');
  const token=account=>JSON.stringify({tokens:{access_token:sign(account),account_id:account}});
  const auth=path.join(codex.root,'.codex','auth.json');write(auth,token('old-fixture'));
  const at=Date.now(),reset=at+7200000;
  const identity=crypto.createHash('sha256').update('kuvo-code plan limits account v1\nchatgpt:old-fixture/fixture-user').digest('hex').slice(0,16);
  write(path.join(codex.home,'agents','limits.json'),JSON.stringify({accounts:{'codex-cli':{identity,plan:'Plus',next:at+600000,checked:at,windows:[{id:'primary',used:0.82,read:at,source:'usage',minutes:300,resetsAt:reset}] }},holds:{}}));
  let listener=await codex.start();const base='kuvo/agents/api/kuvo/limits';
  const old=await until(async()=>{const reply=await request(listener,'GET',base);return reply.json.data.rows.find(r=>r.provider==='codex-cli'&&r.rings?.short?.used===0.82);},'The confirmed legacy reading should be visible before a maker refresh');
  assert.equal(old.rings.short.resetsAt,reset);assert.equal(codex.sent('initialize').length,0);
  await codex.stop();write(auth,token('new-fixture'));listener=await codex.start();
  const newer=await until(async()=>{const reply=await request(listener,'GET',base);return reply.json.data.rows.find(r=>r.provider==='codex-cli'&&r.id!==old.id);},'The newly signed-in account should replace the old account');
  assert.ok(!newer.windows?.some(w=>w.used===0.82));assert.ok(!newer.rings?.short);assert.equal(codex.sent('initialize').length,0);
});


test('Insights groups bot chats, overseen starts and nested subagents without expanding their transcripts',async t=>{
  const {computer}=await bench(t),box=await computer('Lineage',undefined,isolated);
  const at=Date.now()-1000,model={providerID:'claude-code',id:'claude-opus-4-6'};
  const look={color:'indigo',shape:'squircle',face:'calm'};
  write(path.join(box.home,'bots','bot_lead','bot.json'),JSON.stringify({id:'bot_lead',kuvo:1,name:'Lead',look}));
  write(path.join(box.home,'bots','bot_lead','session'),'ses_lead');
  const header=(id,extra={})=>{
    const folder=path.join(box.home,'agents','sessions',id);
    write(path.join(folder,'session.json'),JSON.stringify({id,title:'Title of '+id,model,projectID:'fixture',time:{created:at-2000,updated:at},location:{directory:box.home},...extra}));
    write(path.join(folder,'agent.json'),JSON.stringify({threads:{}}));
    write(path.join(folder,'messages.json'),JSON.stringify([{id:'call-'+id,type:'assistant',time:{created:at-1000,completed:at},model,agent:'build',tokens:{input:100,output:50,cache:{read:500,write:2}},content:[]}]));
  };
  header('ses_lead');header('ses_lead_sub',{parentID:'ses_lead'});
  header('ses_task',{metadata:{kuvo:{startedBy:'ses_lead',by:{bot:'bot_lead',name:'Lead'}}}});
  header('ses_task_sub',{parentID:'ses_task'});header('ses_task_sub_sub',{parentID:'ses_task_sub'});
  header('ses_older',{metadata:{kuvo:{startedBy:'ses_task'}}});
  header('ses_seat',{metadata:{kuvo:{seat:{group:'grp_1',bot:'bot_lead'}}}});
  header('ses_alone');header('ses_other',{metadata:{kuvo:{by:{bot:'bot_other',name:'Other'}}}});
  const call=box.connect(await box.start().ready()),route='kuvo/agents/api/kuvo/insights/summary?period=7&zone=UTC&refresh=1';
  const read=async suffix=>{const answer=await call('GET',route+(suffix??''));assert.equal(answer.status,200,answer.text);return answer.json.data;};
  const all=await read(),bots=all.lists.find(l=>l.key==='bots').rows;
  assert.equal(all.totals.tokens,9*652);const lead=bots.find(r=>r.id==='bot_lead');assert.equal(lead.tokens,7*652);assert.equal(lead.name,'Lead');assert.equal(lead.note,'2 sessions');assert.deepEqual(lead.look,look);
  const children=Object.fromEntries(lead.sessions.map(r=>[r.id,{tokens:r.tokens,note:r.note}]));
  assert.deepEqual(children,{ses_task:{tokens:3*652,note:'2 subagents'},ses_lead:{tokens:2*652,note:'Own chat'},ses_older:{tokens:652,note:undefined},ses_seat:{tokens:652,note:'Own chat'}});
  assert.equal(bots.find(r=>r.id==='ses_alone').note,'No bot');assert.equal(bots.find(r=>r.id==='bot_other').tokens,652);
  const byBot=await read('&bot=bot_lead');assert.equal(byBot.totals.tokens,7*652);assert.deepEqual(byBot.lists.find(l=>l.key==='sessions').rows.map(r=>r.id).sort(),Object.keys(children).sort());
  const bySession=await read('&session=ses_task');assert.equal(bySession.totals.tokens,3*652);assert.deepEqual(bySession.lists.find(l=>l.key==='sessions').rows.map(r=>r.id).sort(),['ses_task','ses_task_sub','ses_task_sub_sub']);
});

test('accepted pace and the emptiest of three named accounts determine cards and rail, even after restart',async t=>{
  if(boxBinary().typescript)return t.skip('The fixture seeds a legacy cache to exercise the Rust restart path.');
  const crypto=await import('node:crypto');
  const codex=await scriptedCodex();t.after(()=>codex.close());
  const latest=Math.floor(Date.now()/300000)*300000,start=latest-30*60000,reset=start+3*3600000;
  const entries={},paces={};
  const seeds=[['default','Fast',0.6,[0.3,0.4,0.5,0.6],'turn'],['a1','Slow',0.33,[0.3,0.31,0.32,0.33],'usage'],['a2','Empty',1,[1],'usage']];
  write(path.join(codex.home,'agents','accounts.json'),JSON.stringify([{formatVersion:1},...seeds.slice(1).map(([id,label])=>({kind:'codex',id,label}))]));
  for(const [id,label,used,readings,source] of seeds){
    const maker='fixture-'+id,user='fixture-user',provider=id==='default'?'codex-cli':'codex-cli@'+id;
    const token=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:maker,chatgpt_user_id:user}})).toString('base64url'),'fixture'].join('.');
    const folder=id==='default'?path.join(codex.root,'.codex'):path.join(codex.home,'agents','codex','accounts',id);
    write(path.join(folder,'auth.json'),JSON.stringify({tokens:{access_token:token,account_id:maker}}));
    const identity=crypto.createHash('sha256').update('kuvo-code plan limits account v1\nchatgpt:'+maker+'/'+user).digest('hex').slice(0,16);
    entries[provider]={identity,plan:id==='default'?'Fast':'Plus',checked:latest,next:latest+600000,windows:[{id:'primary',used,read:latest,source,minutes:300,resetsAt:reset}]};
    paces[`codex#${identity}|||300`]={resetsAt:reset,samples:readings.map((used,i)=>({at:latest-(readings.length-1-i)*600000,used}))};
  }
  write(path.join(codex.home,'agents','limits.json'),JSON.stringify({accounts:entries,paces,holds:{}}));
  let listener=await codex.start();
  const sample=async()=>{
    const response=await request(listener,'GET','kuvo/agents/api/kuvo/limits');assert.equal(response.status,200,response.text);
    const data=response.json.data;const [fast,slow,empty]=['codex-cli','codex-cli@a1','codex-cli@a2'].map(id=>data.rows.find(r=>r.provider===id));
    assert.ok(fast&&slow&&empty);
    assert.equal(fast.card.dot,'warning');assert.equal(fast.card.title,'Codex · Fast');
    assert.deepEqual(fast.card.lines[0].words,[{text:'40% left'},{text:' · ',tone:'faint'},{text:'runs out ~',at:latest+40*60000,as:'time',tone:'warning',strong:true},{text:', resets ',at:reset,as:'time',tone:'muted'}]);
    assert.equal(slow.card.tone,'calm');assert.equal(slow.card.lines[0].words.at(-1).text,'lasts until reset');
    assert.equal(empty.card.dot,'danger');assert.deepEqual(empty.card.lines[0].words,[{text:'Back at ',at:reset,as:'time',tone:'danger',strong:true}]);
    const rail=data.rail.find(r=>r.key==='codex');assert.equal(rail.count,3);assert.equal(rail.ring,empty.id);assert.equal(rail.selected,fast.id);assert.equal(rail.summary,'1 of 3 fine');assert.equal(rail.dot,'danger');assert.deepEqual(rail.rows,[empty.id,fast.id,slow.id]);
    const meter=fast.usage.meters[0];assert.equal(meter.title,'5-hour');assert.equal(meter.readAt,latest);assert.equal(meter.share,0.6);assert.equal(meter.value,'60%');assert.equal(meter.resetsAt,reset);assert.ok(meter.freshForMs>0);
    return {cards:[fast.card,slow.card,empty.card],rail};
  };
  const before=await sample();await codex.stop();listener=await codex.start();assert.deepEqual(await sample(),before);assert.equal(codex.sent('initialize').length,0);
});

test('different provider window names for the same account do not create a second or stale ring (Rust correction)',async t=>{
  if(boxBinary().typescript)return t.skip('The legacy-reading fixture uses Rust’s credential admission; this checks the stale-window correction.');
  const crypto=await import('node:crypto');const codex=await scriptedCodex();t.after(()=>codex.close());
  const maker='same-fixture-account',user='same-fixture-user';
  const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:maker,chatgpt_user_id:user}})).toString('base64url'),'fixture'].join('.');
  const auth=JSON.stringify({tokens:{access_token:jwt,account_id:maker}});
  write(path.join(codex.root,'.codex','auth.json'),auth);
  write(path.join(codex.home,'agents','accounts.json'),JSON.stringify([{formatVersion:1},{kind:'codex',id:'same',label:'Same plan'}]));
  write(path.join(codex.home,'agents','codex','accounts','same','auth.json'),auth);
  const identity=crypto.createHash('sha256').update('kuvo-code plan limits account v1\nchatgpt:'+maker+'/'+user).digest('hex').slice(0,16);
  const at=Date.now()-60000,reset=at+7200000;
  const entry=(id,used,read,resetsAt)=>({identity,plan:'Plus',checked:read,next:at+600000,windows:[{id,used,read,source:'usage',minutes:300,resetsAt}]});
  write(path.join(codex.home,'agents','limits.json'),JSON.stringify({accounts:{'codex-cli':entry('primary',0.17,at,reset),'codex-cli@same':entry('legacy-primary',0.99,at+1000,reset-3600000)},holds:{}}));
  const call=await codex.start();
  const view=async()=>{
    const response=await request(call,'GET','kuvo/agents/api/kuvo/limits');assert.equal(response.status,200,response.text);
    const rows=response.json.data.rows.filter(r=>r.service==='codex');assert.equal(rows.length,1);assert.equal(rows[0].boxes.length,2);
    assert.equal(rows[0].windows.length,1);assert.equal(rows[0].rings.short.used,0.17);assert.equal(rows[0].rings.short.resetsAt,reset);
  };
  await view();assert.equal(codex.sent('initialize').length,0);
});

test('Insights pages share the whole filtered snapshot, list its bots, and refuse invalid pages',async t=>{
  const {computer}=await bench(t),box=await computer('InsightPages',undefined,isolated);
  const at=Date.now()-1000,model={providerID:'claude-code',id:'claude-opus-4-6'};
  for(let i=0;i<51;i++){
    const id='ses_page_'+String(i).padStart(2,'0'),folder=path.join(box.home,'agents','sessions',id);
    write(path.join(folder,'session.json'),JSON.stringify({id,title:'Page '+i,model,projectID:'fixture',time:{created:at-1000,updated:at},location:{directory:box.home},metadata:{kuvo:{by:{bot:'bot_fixture',name:'Fixture bot'}}}}));
    write(path.join(folder,'agent.json'),JSON.stringify({threads:{}}));
    write(path.join(folder,'messages.json'),JSON.stringify([{id:'call-'+id,type:'assistant',time:{created:at-1000,completed:at},model,agent:'build',tokens:{input:100,output:50,cache:{read:500,write:2}},content:[]}]));
  }
  const {sessionOf}=await import('./engine.mjs');box.engine.sessions.push(sessionOf({id:'ses_page_50',title:'Page 50',directory:box.home,updated:at}));
  const call=box.connect(await box.start().ready()),base='kuvo/agents/api/kuvo/insights';
  assert.equal((await call('GET','kuvo/sessions/list')).status,200);
  const ready=await call('GET',base+'/summary?period=7&zone=UTC&refresh=1');assert.equal(ready.status,200,ready.text);assert.equal(ready.json.data.totals.tokens,51*652);
  const overview=await call('GET',base+'?period=7&zone=UTC');assert.equal(overview.status,200,overview.text);assert.equal(overview.json.data.totals.sessions,51);assert.deepEqual(overview.json.data.bots,[{key:'bot_fixture',name:'Fixture bot'}]);
  const first=await call('GET',base+'/sessions?period=7&zone=UTC&bot=bot_fixture');assert.equal(first.status,200,first.text);assert.equal(first.json.data.sessions.length,50);assert.equal(first.json.data.next,50);assert.ok(first.json.data.sessions.every(row=>row.tokenShare===1/51));
  const second=await call('GET',base+'/sessions?period=7&zone=UTC&bot=bot_fixture&offset=50');assert.equal(second.status,200,second.text);assert.equal(second.json.data.sessions.length,1);assert.equal(second.json.data.next,null);assert.equal(second.json.data.sessions[0].tokenShare,1/51);
  const detail=await call('GET',base+'/sessions/ses_page_50?period=7&zone=UTC');assert.equal(detail.status,200,detail.text);assert.equal(detail.json.data.calls.length,1);assert.equal(detail.json.data.calls[0].tokenCount,152);assert.equal(detail.json.data.next,null);
  await until(async()=>{const answer=await call('GET',base+'/sessions/ses_page_50?period=7&zone=UTC');return answer.json?.data?.destination?.status==='available';},'The known session owner did not become available');
  const none=await call('GET',base+'/sessions/ses_missing?period=7&zone=UTC');assert.equal(none.status,200,none.text);assert.deepEqual(none.json.data.destination,{status:'unavailable'});
  for(const [route,message] of [[base+'/summary?period=365','Choose a 7, 30 or 90 day period.'],[base+'/sessions?offset=-1','Choose a 7, 30, 90 or 365 day period and a valid page.'],[base+'?period=11','Choose a 7, 30, 90 or 365 day period and a valid page.']]){
    const answer=await call('GET',route);assert.equal(answer.status,400,answer.text);assert.equal(answer.json.message,message);
  }
});

test('Insights keeps every unowned session beyond the former ranked-list cap',async t=>{
  const fs=await import('node:fs');const {DatabaseSync}=await import('node:sqlite');
  const {computer}=await bench(t),box=await computer('InsightUnowned',undefined,isolated);
  const folder=path.join(box.home,'data','opencode');fs.mkdirSync(folder,{recursive:true});
  const db=new DatabaseSync(path.join(folder,'opencode.db'));
  db.exec('CREATE TABLE session_v2(id TEXT PRIMARY KEY,title TEXT,parent_id TEXT,metadata TEXT);CREATE TABLE session_message(id TEXT PRIMARY KEY,session_id TEXT,type TEXT,time_created INTEGER,time_updated INTEGER,data TEXT)');
  const at=Date.now()-1000,body=JSON.stringify({time:{completed:at},model:{providerID:'claude',id:'claude-opus-4-6'},tokens:{input:100,output:50,cache:{read:500,write:2}}});
  const header=db.prepare('INSERT INTO session_v2 VALUES (?,?,?,?)'),message=db.prepare('INSERT INTO session_message VALUES (?,?,?,?,?,?)');
  db.exec('BEGIN');for(let i=0;i<201;i++){const id='loose-'+i;header.run(id,'Loose '+i,null,'{}');message.run('call-'+i,id,'assistant',at,at,body);}db.exec('COMMIT');db.close();
  const call=box.connect(await box.start().ready());const answer=await call('GET','kuvo/agents/api/kuvo/insights/summary?period=7&zone=UTC&refresh=1');assert.equal(answer.status,200,answer.text);
  const summary=answer.json.data,rows=summary.lists.find(list=>list.key==='bots').rows;
  assert.equal(rows.length,201);assert.equal(new Set(rows.map(row=>row.id)).size,201);assert.ok(rows.every(row=>row.note==='No bot'));
  assert.equal(rows.reduce((sum,row)=>sum+row.tokens,0),summary.totals.tokens);assert.equal(summary.totals.tokens,201*652);
});

test('Insights lists only completed skill use from compact portable and engine records',async t=>{
  const fs=await import('node:fs'),{DatabaseSync}=await import('node:sqlite');
  const {computer}=await bench(t),box=await computer('InsightSkills',undefined,isolated),at=Date.now()-1000;
  const model={providerID:'claude-code',id:'claude-opus-4-6'},tokens={input:100,output:50,cache:{read:500,write:2}};
  const tool=(name,input,status='completed')=>({type:'tool',id:name,name,state:{status,input,content:[]}});
  const content=[tool('Skill',{skill:'research'}),tool('Read',{filePath:'.agents/skills/fixture/SKILL.md'}),tool('read_file',{path:'C:\\scratch\\skills\\fixture\\SKILL.md'}),tool('Skill',{name:'ignored'},'running'),tool('Read',{path:'not-a-skill.md'})];
  const id='ses_skill_portable',portable=path.join(box.home,'agents','sessions',id);
  write(path.join(portable,'session.json'),JSON.stringify({id,title:'Portable skills',model,projectID:'fixture',time:{created:at-1000,updated:at},location:{directory:box.home}}));
  write(path.join(portable,'agent.json'),JSON.stringify({threads:{}}));
  write(path.join(portable,'messages.json'),JSON.stringify([{id:'call-portable',type:'assistant',time:{created:at-1000,completed:at},model,agent:'build',tokens,content}]));
  const folder=path.join(box.home,'data','opencode');fs.mkdirSync(folder,{recursive:true});const db=new DatabaseSync(path.join(folder,'opencode.db'));
  db.exec('CREATE TABLE session_v2(id TEXT PRIMARY KEY,title TEXT,parent_id TEXT,metadata TEXT);CREATE TABLE session_message(id TEXT PRIMARY KEY,session_id TEXT,type TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);CREATE TABLE session(id TEXT PRIMARY KEY,title TEXT,parent_id TEXT,metadata TEXT);CREATE TABLE message(id TEXT PRIMARY KEY,session_id TEXT,time_created INTEGER,time_updated INTEGER,data TEXT);CREATE TABLE part(message_id TEXT,data TEXT)');
  db.prepare('INSERT INTO session_v2 VALUES(?,?,?,?)').run('ses_skill_v2','V2',null,'{}');
  db.prepare('INSERT INTO session VALUES(?,?,?,?)').run('ses_skill_v1','V1',null,'{}');
  const engine={model:{providerID:'claude',id:model.id},time:{completed:at},tokens};
  db.prepare('INSERT INTO session_message VALUES(?,?,?,?,?,?)').run('call-v2','ses_skill_v2','assistant',at,at,JSON.stringify({...engine,content}));
  db.prepare('INSERT INTO message VALUES(?,?,?,?,?)').run('call-v1','ses_skill_v1',at,at,JSON.stringify({...engine,role:'assistant'}));
  for(const part of content)db.prepare('INSERT INTO part VALUES(?,?)').run('call-v1',JSON.stringify(part));db.close();
  const call=box.connect(await box.start().ready()),base='kuvo/agents/api/kuvo/insights';
  assert.equal((await call('GET',base+'/summary?period=7&zone=UTC&refresh=1')).json.data.totals.tokens,3*652);
  for(const id of ['ses_skill_portable','ses_skill_v1','ses_skill_v2']){
    const answer=await call('GET',base+'/sessions/'+id+'?period=7&zone=UTC');assert.equal(answer.status,200,answer.text);assert.equal(answer.json.data.calls.length,1);assert.deepEqual(answer.json.data.calls[0].skills,['fixture','research']);
  }
  const overview=await call('GET',base+'?period=7&zone=UTC');const skills=overview.json.data.breakdowns.skill;assert.deepEqual(skills.map(row=>row.name).sort(),['fixture','research']);assert.ok(skills.every(row=>row.processed===3*652));
});

test('engine credentials omit inactive keys, read the Z.ai alias, and distinguish no-plan from lapsed sign-ins',async t=>{
  if(boxBinary().typescript)return t.skip('Rust reads the isolated engine database rather than starting a maker.');
  const fs=await import('node:fs');
  const http=await import('node:http');
  const {DatabaseSync}=await import('node:sqlite');
  const now=Date.now();
  const seen=[];
  const maker=http.createServer((req,res)=>{
    seen.push([req.url,req.headers.authorization]);
    const reply=req.url==='/zai' ? {data:{limits:[{type:'TOKENS_LIMIT',unit:3,number:5,percentage:12,nextResetTime:now+7200000}]}} : {type:'error',error:{type:'EntitlementError',message:'No active subscription'}};
    res.writeHead(req.url==='/zai'?200:403,{'Content-Type':'application/json'});res.end(JSON.stringify(reply));
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('EngineAliases',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const folder=path.join(box.home,'data','opencode');fs.mkdirSync(folder,{recursive:true});
  const sqlite=path.join(folder,'opencode.db');
  let db=new DatabaseSync(sqlite);
  db.exec('CREATE TABLE credential(id TEXT,integration_id TEXT,value TEXT,active INTEGER,time_updated INTEGER)');
  const insert=db.prepare('INSERT INTO credential VALUES(?,?,?,?,?)');
  for(const [id,value,active] of [
    ['openai',{type:'api',key:'fixture-not-a-plan'},1],
    ['zai',{type:'api',key:'fixture-zai-alias'},1],
    ['zai-coding-plan',{type:'api',key:'fixture-inactive'},null],
    ['opencode-go',{type:'api',key:'fixture-no-go-plan'},1],
    ['xai',{type:'oauth',access:'fixture-expired',expires:now-1},1],
  ])insert.run(id,id,JSON.stringify(value),active,now);
  db.close();
  fs.unlinkSync(path.join(box.home,'run','engine.json'));
  const before=fs.readFileSync(sqlite);
  const call=box.connect(await box.start().ready());
  const refreshed=await call('POST','kuvo/agents/api/kuvo/limits/refresh',{});
  assert.equal(refreshed.status,200,refreshed.text);
  const data=await until(async()=>{
    const reply=await call('GET','kuvo/agents/api/kuvo/limits');
    return !reply.json.data.refreshPending&&reply.json.data;
  },'Engine credential read did not settle');
  const accounts=Object.fromEntries(data.accounts.map(a=>[a.provider,a]));
  assert.deepEqual(Object.keys(accounts).sort(),['xai','zai']);
  assert.equal(accounts.zai.service,'zai');
  assert.equal(accounts.zai.windows[0].used,0.12);
  assert.equal(accounts.xai.problem.kind,'expired');
  assert.deepEqual(seen.sort((a,b)=>a[0].localeCompare(b[0])),[['/opencodeGo','Bearer fixture-no-go-plan'],['/zai','Bearer fixture-zai-alias']]);
  assert.equal(data.refreshFailed,false);
  assert.deepEqual(fs.readFileSync(sqlite),before);
  assert.equal(box.engine.asked.filter(r=>r.path==='/api/credential').length,0);
});

test('an engine maker’s dated hold survives restart without fetching again',async t=>{
  if(boxBinary().typescript)return t.skip('The scratch bridge disables engine provider reads.');
  const fs=await import('node:fs');
  const http=await import('node:http');
  const {DatabaseSync}=await import('node:sqlite');
  const now=Date.now(),untilAt=Math.floor((now+3600000)/1000)*1000;
  let reads=0;
  const maker=http.createServer((req,res)=>{
    reads++;
    res.writeHead(429,{'Content-Type':'application/json','Retry-After':new Date(untilAt).toUTCString()});res.end('{}');
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('EngineHeld',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const folder=path.join(box.home,'data','opencode');fs.mkdirSync(folder,{recursive:true});
  const db=new DatabaseSync(path.join(folder,'opencode.db'));
  db.exec('CREATE TABLE credential(id TEXT,integration_id TEXT,value TEXT,active INTEGER,time_updated INTEGER)');
  db.prepare('INSERT INTO credential VALUES(?,?,?,?,?)').run('one','openrouter',JSON.stringify({type:'api',key:'fixture-held'}),1,now);db.close();
  fs.unlinkSync(path.join(box.home,'run','engine.json'));
  let resident=box.start(),call=box.connect(await resident.ready());
  const refresh=async()=>{
    const reply=await call('POST','kuvo/agents/api/kuvo/limits/refresh',{});
    assert.equal(reply.status,200,reply.text);
    return until(async()=>{const view=await call('GET','kuvo/agents/api/kuvo/limits');return !view.json.data.refreshPending&&view.json.data;},'Engine hold did not settle');
  };
  let data=await refresh();
  assert.equal(data.accounts.find(a=>a.provider==='openrouter').problem.kind,'rate-limited');
  assert.equal(data.accounts.find(a=>a.provider==='openrouter').problem.until,untilAt);
  assert.equal(reads,1);
  await resident.stop();resident=box.start();call=box.connect(await resident.ready());
  data=await refresh();
  assert.equal(data.accounts.find(a=>a.provider==='openrouter').problem.until,untilAt);
  assert.equal(reads,1);
});

async function engineQuotaFixture(t,name,respond,credentials) {
  const http=await import('node:http');
  const seen=[];
  const maker=http.createServer((req,res)=>{
    seen.push({path:req.url,auth:req.headers.authorization});
    const reply=respond(req);
    const send=()=>{res.writeHead(reply.status??200,{'Content-Type':'application/json',...reply.headers});
    res.end(JSON.stringify(reply.body??{}));};
    if(reply.delay)setTimeout(send,reply.delay);else send();
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer(name,undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  box.engine.credentials=credentials;
  const resident=box.start(),listener=await resident.ready(),call=box.connect(listener);
  const refresh=async()=>{
    const result=await call('POST','kuvo/agents/api/kuvo/limits/refresh',{});
    assert.equal(result.status,200,result.text);
    return until(async()=>{const reply=await call('GET','kuvo/agents/api/kuvo/limits');return !reply.json.data.refreshPending&&reply.json.data;},'Engine provider refresh did not settle');
  };
  return {box,seen,refresh,call,listener,resident};
}
const engineSign=(integrationID,value)=>({integrationID,active:true,value});

test('OpenRouter spend periods end at the UTC day, Monday and month boundaries, with no fabricated zero',async t=>{
  if(boxBinary().typescript)return t.skip('The scratch bridge disables engine provider reads.');
  let complete=true;
  const now=Date.now(),today=new Date(now);today.setUTCHours(0,0,0,0);
  const week=new Date(today);week.setUTCDate(week.getUTCDate()+7-((week.getUTCDay()+6)%7));
  const month=new Date(today);month.setUTCDate(1);month.setUTCMonth(month.getUTCMonth()+1);
  const {refresh}=await engineQuotaFixture(t,'RouterPeriods',req=>({body:req.url==='/openrouterKey'?{data:complete?{limit:null,usage_daily:1.84,usage_weekly:9.2,usage_monthly:31.75,is_free_tier:false}:{limit:10,usage:2}}:{data:{total_credits:50,total_usage:42.3}}}),[engineSign('openrouter',{type:'api',key:'fixture-router'})]);
  let account=(await refresh()).accounts.find(a=>a.provider==='openrouter');
  assert.deepEqual(account.balance,{left:50-42.3,limit:50,unit:'usd'});
  assert.deepEqual(account.extras,[
    {kind:'spent',id:'today',name:'Today',amount:1.84,unit:'usd',resetsAt:today.getTime()+86400000},
    {kind:'spent',id:'week',name:'This Week',amount:9.2,unit:'usd',resetsAt:week.getTime()},
    {kind:'spent',id:'month',name:'This Month',amount:31.75,unit:'usd',resetsAt:month.getTime()},
  ]);
  complete=false;
  account=(await refresh()).accounts.find(a=>a.provider==='openrouter');
  assert.equal(account.extras,undefined);
});

test('Copilot organization credits ignore seat fees, retain usable answers through failure and stop on a GitHub hold',async t=>{
  if(boxBinary().typescript)return t.skip('The scratch bridge disables engine provider reads.');
  let mode='hold';
  const sign=refresh=>engineSign('github-copilot',{type:'oauth',access:'short',refresh,expires:Date.now()-1000});
  const orgPrefix='/githubBilling/';
  const {box,refresh,seen,call}=await engineQuotaFixture(t,'CopilotOrg',req=>{
    if(req.url==='/copilot')return {body:{login:'fixture-admin',copilot_plan:'business',token_based_billing:true,quota_snapshots:{premium_interactions:{entitlement:0,remaining:0,percent_remaining:0}}}};
    if(req.url==='/githubOrgs')return {body:[{login:'member-only'},{login:'acme'}]};
    if(req.url.startsWith(orgPrefix+'member-only/'))return {status:403,headers:mode==='hold'?{'Retry-After':'60'}:{},body:{message:'Must have admin rights'}};
    if(req.url.startsWith(orgPrefix+'acme/')){
      if(mode==='failed')return {status:503};
      if(mode==='member')return {status:403};
      return {body:{usageItems:[
        {product:'Copilot',unitType:'ai-credits',grossQuantity:1200,netAmount:14.5},
        {product:'copilot',unitType:'AI-Units',grossQuantity:300,netAmount:0.5},
        {product:'Copilot',unitType:'user-months',grossQuantity:3,netAmount:57},
        {product:'Actions',unitType:'minutes',grossQuantity:900,netAmount:2},
      ]}};
    }
    return {status:404};
  },[sign('fixture-admin')]);
  let account=(await refresh()).accounts[0];
  assert.equal(account.extras,undefined);
  assert.deepEqual(seen.filter(r=>r.path.startsWith(orgPrefix)).map(r=>r.path.split('/')[2]),['member-only']);
  mode='allowed';
  account=(await refresh()).accounts[0];
  const expected=[{kind:'spent',id:'org_credits',name:'Org Credits',amount:1500,unit:'credits'},{kind:'spent',id:'org_spend',name:'Org Spend',amount:15,unit:'usd'}];
  assert.deepEqual(account.windows,[]);
  assert.deepEqual(account.extras,expected);
  assert.equal(seen.find(r=>r.path==='/githubOrgs').auth,'token fixture-admin');
  mode='failed';account=(await refresh()).accounts[0];assert.deepEqual(account.extras,expected);
  mode='member';box.engine.credentials=[sign('fixture-member')];
  account=(await refresh()).accounts[0];
  assert.equal(account.plan,'Business');
  assert.equal(account.extras,undefined);
  box.engine.credentials=[];
  const asked=seen.length;
  const signedOut=await call('GET','kuvo/agents/api/kuvo/limits');
  assert.ok(!signedOut.json.data.accounts.some(a=>a.provider==='github-copilot'));
  assert.equal(seen.length,asked,'Withdrawing a legacy engine sign-in requires no maker request');
});

test('a malformed SuperGrok answer retains the previous window, but an ended plan removes all rings',async t=>{
  if(boxBinary().typescript)return t.skip('The scratch bridge disables engine provider reads.');
  let mode='active';
  const now=Date.now(),start=new Date(now-3*86400000).toISOString(),end=new Date(now+4*86400000).toISOString();
  const jwt=['e30',Buffer.from(JSON.stringify({sub:'fixture-grok'})).toString('base64url'),'fixture'].join('.');
  const {refresh}=await engineQuotaFixture(t,'GrokEnded',()=>({body:{config:mode==='active'?{currentPeriod:{start,end},creditUsagePercent:40}:mode==='malformed'?{current_period:{start,end},creditUsagePercent:20}:{isUnifiedBillingUser:true}}}),[engineSign('xai',{type:'oauth',access:jwt,expires:now+3600000})]);
  let data=await refresh();
  assert.equal(data.rows.find(r=>r.provider==='xai').rings.long.used,0.4);
  mode='malformed';data=await refresh();
  assert.equal(data.accounts[0].problem.kind,'failed');
  assert.equal(data.rows.find(r=>r.provider==='xai').rings.long.used,0.4);
  mode='ended';data=await refresh();
  const row=data.rows.find(r=>r.provider==='xai');
  assert.equal(row.ended,true);
  assert.equal(row.problem.kind,'ended');
  assert.deepEqual(row.windows,[]);
  assert.deepEqual(row.rings,{});
});

test('a turned-away engine OAuth sign-in waits for renewal of that credential, then reads the same account',async t=>{
  if(boxBinary().typescript)return t.skip('The scratch bridge disables engine provider reads.');
  const fs=await import('node:fs');
  const http=await import('node:http');
  const {DatabaseSync}=await import('node:sqlite');
  const now=Date.now();
  const claim=Buffer.from(JSON.stringify({sub:'fixture-renewing-grok'})).toString('base64url');
  const sign=signature=>({type:'oauth',access:['e30',claim,signature].join('.'),expires:now+3600000});
  let allowed=false,reads=0;
  const maker=http.createServer((req,res)=>{
    reads++;
    const body=allowed?{config:{currentPeriod:{start:new Date(now-3*86400000).toISOString(),end:new Date(now+4*86400000).toISOString()},creditUsagePercent:37}}:{};
    res.writeHead(allowed?200:401,{'Content-Type':'application/json'});res.end(JSON.stringify(body));
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('RenewedEngine',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const folder=path.join(box.home,'data','opencode');fs.mkdirSync(folder,{recursive:true});
  const sqlite=path.join(folder,'opencode.db');
  let db=new DatabaseSync(sqlite);db.exec('CREATE TABLE credential(id TEXT,integration_id TEXT,value TEXT,active INTEGER,time_updated INTEGER)');
  db.prepare('INSERT INTO credential VALUES(?,?,?,?,?)').run('one','xai',JSON.stringify(sign('old')),1,now);db.close();
  fs.unlinkSync(path.join(box.home,'run','engine.json'));
  const call=box.connect(await box.start().ready());
  const refresh=async()=>{
    const reply=await call('POST','kuvo/agents/api/kuvo/limits/refresh',{});
    assert.equal(reply.status,200,reply.text);
    return until(async()=>{const view=await call('GET','kuvo/agents/api/kuvo/limits');return !view.json.data.refreshPending&&view.json.data;},'Renewed engine plan did not settle');
  };
  let account=(await refresh()).accounts[0];
  assert.equal(account.problem.kind,'expired');
  const identity=account.identity;
  assert.ok(identity);
  account=(await refresh()).accounts[0];
  assert.equal(account.problem.kind,'expired');
  assert.equal(reads,1,'A refused OAuth credential must not be sent again unchanged');
  allowed=true;
  db=new DatabaseSync(sqlite);db.prepare('UPDATE credential SET value=?,time_updated=? WHERE id=?').run(JSON.stringify(sign('new')),now+1,'one');db.close();
  account=(await refresh()).accounts[0];
  assert.equal(account.identity,identity);
  assert.equal(account.problem,undefined);
  assert.equal(account.windows[0].used,0.37);
  assert.equal(reads,2);
});

test('a connected menu borrows a fresh account reading from another computer without asking the maker again',{timeout:120000},async t=>{
  if(boxBinary().typescript)return t.skip('Scratch TS provider reads are disabled; the Rust reader must reuse actual peer mail.');
  const http=await import('node:http');
  const {openLine}=await import('./adapter.mjs');
  const now=Date.now(),reset=Math.floor(now/1000)+7200;
  const asked=[];
  const maker=http.createServer((req,res)=>{
    asked.push(req.headers.authorization);
    res.writeHead(200,{'Content-Type':'application/json'});
    res.end(JSON.stringify({plan_type:'plus',rate_limit:{primary_window:{used_percent:req.headers.authorization==='Bearer fixture-studio'?23:91,limit_window_seconds:18000,reset_at:reset}}}));
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer,leases}=await bench(t);
  const env=folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'});
  const studio=await computer('FreshStudio',undefined,env);
  const lab=await computer('FreshLab',undefined,env);
  const sign=access=>[{integrationID:'openai',active:true,value:{type:'oauth',access,expires:now+3600000,metadata:{accountID:'same-fresh-account'}}}];
  studio.engine.credentials=sign('fixture-studio');lab.engine.credentials=sign('fixture-lab');
  const base='kuvo/agents/api/kuvo/limits';
  const main=studio.connect(await studio.start().ready());
  const read=await main('POST',base+'/refresh',{});
  assert.equal(read.status,200,read.text);
  const original=await until(async()=>{
    const answer=await main('GET',base);
    return answer.json.data.rows.find(r=>r.provider==='openai'&&r.rings.short?.used===0.23);
  },'Studio did not read the plan');
  assert.deepEqual(asked,['Bearer fixture-studio']);
  const setup=await main('GET','kuvo/setup');
  const invite=await main('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});
  assert.equal(invite.status,201,invite.text);
  await until(()=>leases.leaseFor(setup.json.setup),'Studio did not claim its relay lease');
  const joined=await lab.join(invite.json.code);assert.equal(joined.code,0,joined.log);
  let resident=lab.start();
  const listener=await resident.ready();
  let call=lab.connect(listener);
  await until(async()=>{const data=(await call('GET',base)).json.data;return data.rows.find(r=>r.id===original.id)?.rings.short?.used===0.23;},'Lab did not receive fresh peer evidence',30000,100);
  const line=await openLine(listener,'','kuvo/box/at-work/events',{},30000);
  try{
    assert.equal(line.status,200);
    const borrowed=await until(async()=>{
      const data=(await call('GET',base)).json.data;
      return !data.refreshPending&&data.accounts.find(a=>a.provider==='openai'&&a.windows.length)&&data;
    },'Opening the menu did not admit the local holder',20000,100);
    assert.deepEqual(asked,['Bearer fixture-studio'],'A fresh peer reading replaces the passive maker request');
    const own=borrowed.accounts.find(a=>a.provider==='openai');
    assert.equal(own.windows[0].used,0.23);
    assert.equal(own.checked,undefined,'Borrowed numbers are not reported as a local maker read');
    const row=borrowed.rows.find(r=>r.id===original.id);
    assert.equal(row.rings.short.used,0.23);
    assert.equal(row.boxes.length,2);
    assert.equal(row.boxes.find(b=>b.here).checked,undefined);
    await until(async()=>{const data=(await main('GET',base)).json.data;return data.rows.find(r=>r.id===original.id)?.boxes.length===2;},'The borrower was not advertised as a current holder');
  }finally{line.close();}
  await resident.stop();resident=lab.start();call=lab.connect(await resident.ready());
  await until(async()=>{
    const view=(await call('GET',base)).json.data;
    const local=view.accounts.find(a=>a.provider==='openai');
    return local?.windows[0]?.used===0.23 && local.checked===undefined;
  },'The borrowed reading did not survive restart');
  lab.engine.credentials=[];
  const signedOut=(await call('GET',base)).json.data;
  assert.ok(!signedOut.accounts.some(a=>a.provider==='openai'));
  assert.ok(signedOut.rows.find(r=>r.id===original.id)?.boxes.every(b=>!b.here));
  assert.deepEqual(asked,['Bearer fixture-studio'],'Borrowed history is not a sign-in');
});

test('a mixed provider refresh succeeds, and a wholly failed refresh retains its last successful completion',async t=>{
  if(boxBinary().typescript)return t.skip('The scratch bridge disables engine provider reads.');
  let mode='good';
  const {refresh}=await engineQuotaFixture(t,'RefreshSuccess',req=>{
    if(req.url.startsWith('/openrouter')){
      if(mode!=='good')return {status:503};
      return {body:req.url==='/openrouterKey'?{data:{limit:10,usage:2}}:{data:{total_credits:50,total_usage:10}}};
    }
    if(mode==='failed')return {status:503};
    return {body:{data:{limits:[{type:'TOKENS_LIMIT',unit:3,number:5,percentage:12,nextResetTime:Date.now()+7200000}]}}};
  },[engineSign('openrouter',{type:'api',key:'fixture-router'}),engineSign('zai-coding-plan',{type:'api',key:'fixture-zai'})]);
  let data=await refresh();
  assert.equal(data.refreshFailed,false);
  const first=data.refreshCompletedAt;assert.ok(first);
  mode='mixed';data=await refresh();
  assert.equal(data.accounts.find(a=>a.provider==='openrouter').problem.kind,'failed');
  assert.equal(data.accounts.find(a=>a.provider==='zai-coding-plan').problem,undefined);
  assert.equal(data.refreshFailed,false,'A round with a real success is not a failed refresh');
  const latest=data.refreshCompletedAt;assert.ok(latest>=first);
  mode='failed';data=await refresh();
  assert.equal(data.refreshFailed,true);
  assert.equal(data.refreshCompletedAt,latest,'A failure must not make the old success look newer');
});

test('a slow maker keeps the previous ring while refresh is pending and publishes completion on the existing feed',async t=>{
  if(boxBinary().typescript)return t.skip('The scratch bridge disables engine provider reads.');
  let slow=false;
  const {refresh,call,listener}=await engineQuotaFixture(t,'SlowRefresh',()=>({
    delay:slow?3000:0,
    body:{data:{limits:[{type:'TOKENS_LIMIT',unit:3,number:5,percentage:slow?40:12,nextResetTime:Date.now()+7200000}]}}
  }),[engineSign('zai-coding-plan',{type:'api',key:'fixture-zai'})]);
  const first=await refresh();
  assert.equal(first.accounts[0].windows[0].used,0.12);
  const {openLine}=await import('./adapter.mjs');
  const base='kuvo/agents/api/kuvo/limits';
  const line=await openLine(listener,'',base+'/events',{},15000);
  try{
    assert.equal(line.status,200);
    slow=true;
    const pending=await call('POST',base+'/refresh',{});
    assert.equal(pending.status,200,pending.text);
    assert.equal(pending.json.data.refreshPending,true);
    assert.equal(pending.json.data.accounts[0].windows[0].used,0.12);
    let completed;
    for await(const frame of line.frames()){
      if(!frame.changed)continue;
      const view=(await call('GET',base)).json.data;
      if(!view.refreshPending&&view.accounts[0]?.windows[0]?.used===0.4){completed=view;break;}
    }
    assert.ok(completed,'The limits feed did not announce the completed reading');
    assert.equal(completed.refreshFailed,false);
    assert.ok(completed.refreshCompletedAt>=first.refreshCompletedAt);
  }finally{line.close();}
});

test('the shared Grok corpus counts completed model turns once and keeps append-only history through restart',async t=>{
  const fs=await import('node:fs');
  const fixture=name=>new URL('../../box/test/fixtures/usage/'+name,import.meta.url);
  const expected=JSON.parse(fs.readFileSync(fixture('grok-expected.json'),'utf8'));
  const utcDay=at=>new Date(at).toISOString().slice(0,10);
  const shift=(Math.floor(Date.now()/86400000)-1)*86400000-Date.parse('2026-11-12T00:00:00Z');
  const shiftDay=day=>utcDay(Date.parse(day+'T00:00:00Z')+shift);
  const {computer}=await bench(t);
  const box=await computer('GrokCorpus',undefined,isolated);
  write(path.join(box.home,'agents','accounts.json'),JSON.stringify([{formatVersion:1},{kind:'grok',id:'work',label:'Work'}]));
  const home=path.join(box.home,'agents','grok','accounts','work');
  const priceFolder=path.join(box.home,'agents','usage','pricing');
  write(path.join(priceFolder,'litellm.json'),fs.readFileSync(fixture('pricing-grok-litellm.json')));
  write(path.join(priceFolder,'models_dev.json'),fs.readFileSync(fixture('pricing-grok-models-dev.json')));
  const files=[];
  for(const name of expected.files){
    const target=path.join(home,...name.split('/').slice(1));files.push(target);
    const shifted=fs.readFileSync(fixture(name),'utf8').trimEnd().split('\n').map(line=>{
      let row;try{row=JSON.parse(line);}catch{return line;}
      for(const meta of [row._meta,row.params?._meta])if(typeof meta?.agentTimestampMs==='number')meta.agentTimestampMs+=shift;
      if(typeof row.timestamp==='number')row.timestamp+=shift/1000;
      if(typeof row.timestamp==='string')row.timestamp=new Date(Date.parse(row.timestamp)+shift).toISOString();
      return JSON.stringify(row);
    });
    write(target,shifted.join('\n')+'\n');
  }
  // Other files and the CLI debug log are not usage inputs.
  write(path.join(home,'sessions','not-updates.jsonl'),fs.readFileSync(files[0]));
  write(path.join(home,'grok.log'),fs.readFileSync(files[0]));
  let resident=box.start(),call=box.connect(await resident.ready());
  const base='kuvo/agents/api/kuvo/usage';
  const refresh=async()=>{const r=await call('POST',base+'/refresh?zone=UTC',{});assert.equal(r.status,200,r.text);};
  const get=async()=>{
    const r=await call('GET',base+'?zone=UTC');assert.equal(r.status,200,r.text);return r.json.data;
  };
  await refresh();
  const first=await until(async()=>{
    const data=await get();
    return data.rows.find(r=>r.provider==='grok-cli@work')?.days.some(d=>d.models.length)&&data;
  },'The native Grok corpus did not reach usage');
  const rows=first.rows.filter(r=>r.service==='grok'&&r.days.some(d=>d.models.length||d.unknown));
  assert.equal(rows.length,1);
  assert.equal(rows[0].provider,'grok-cli@work');
  assert.equal(rows[0].label,'Work');
  const days=first.totals.grok.filter(d=>d.models.length).map(({date,tokens,cost,models})=>({date,tokens,cost,models}));
  // UsageKit's fixture rounds this one decimal; the bridge's wire value is
  // the exact sum of the two per-token price terms, not that rounded decimal.
  const exact=expected.days.map(d=>({...d,date:shiftDay(d.date),models:d.models.map(m=>m.model==='grok-5-mini'?{...m,cost:1000*0.3/1e6+1000*0.5/1e6}:m)}));
  assert.deepEqual(days,exact);
  assert.deepEqual(Object.fromEntries(first.totals.grok.filter(d=>d.unknown).map(d=>[d.date,d.unknown])),Object.fromEntries(Object.entries(expected.unknown).map(([date,models])=>[shiftDay(date),models])));
  const at=Date.parse('2026-11-12T13:30:00Z')+shift;
  fs.appendFileSync(files.at(-1),JSON.stringify({params:{update:{sessionUpdate:'turn_completed',usage:{modelUsage:{'grok-5':{inputTokens:40,outputTokens:2,costUsdTicks:10000000}}}},_meta:{eventId:'fixture-new',agentTimestampMs:at}}})+'\n');
  await refresh();
  const grown=await until(async()=>{const data=await get();const day=data.totals.grok.find(d=>d.date===shiftDay('2026-11-12'));return day?.tokens===days[0].tokens+42&&day;},'Appended Grok turn did not reach usage');
  assert.equal(grown.cost,days[0].cost+0.001);
  await resident.stop();resident=box.start();call=box.connect(await resident.ready());
  await until(async()=>{const day=(await get()).totals.grok.find(d=>d.date===grown.date);return day?.tokens===grown.tokens;},'Grok history did not survive restart');
  assert.deepEqual((await get()).totals.grok.find(d=>d.date===grown.date),grown);
});

test('the shared Claude corpus prices canonical replies, advisor calls and fast usage without counting replays',async t=>{
  const fs=await import('node:fs');
  const fixture=name=>new URL('../../box/test/fixtures/usage/'+name,import.meta.url);
  const expected=JSON.parse(fs.readFileSync(fixture('claude-expected.json'),'utf8'));
  const today=Math.floor(Date.now()/86400000)*86400000;
  const shift=today-86400000-Date.parse('2026-11-12T00:00:00Z');
  const shiftDay=day=>new Date(Date.parse(day+'T00:00:00Z')+shift).toISOString().slice(0,10);
  const {computer}=await bench(t);
  const box=await computer('ClaudeCorpus',undefined,isolated);
  const home=path.join(box.home,'agents','claude','accounts','work');
  write(path.join(box.home,'agents','accounts.json'),JSON.stringify([{formatVersion:1},{kind:'claude',id:'work',label:'Work'}]));
  const prices=path.join(box.home,'agents','usage','pricing');
  for(const [from,to] of [['pricing-litellm.json','litellm.json'],['pricing-models-dev.json','models_dev.json'],['pricing-supplement.json','supplement.json']])write(path.join(prices,to),fs.readFileSync(fixture(from)));
  for(const name of expected.files){
    const shifted=fs.readFileSync(fixture(name),'utf8').trimEnd().split('\n').map(line=>{
      let row;try{row=JSON.parse(line);}catch{return line;}
      if(typeof row.timestamp==='string'){
        const original=row.timestamp;
        // Keep the corpus's offset/fractional/zone-less timestamp forms, but
        // move its pre-cutoff reply outside the actual 31-day route window.
        const day=original.slice(0,10);
        const date=day<'2026-11-01'?new Date(today-31*86400000).toISOString().slice(0,10):shiftDay(day);
        row.timestamp=date+original.slice(10);
      }
      return JSON.stringify(row);
    });
    write(path.join(home,...name.split('/').slice(1)),shifted.join('\n')+'\n');
  }
  const call=box.connect(await box.start().ready());
  const base='kuvo/agents/api/kuvo/usage';
  assert.equal((await call('POST',base+'/refresh?zone=UTC',{})).status,200);
  const row=await until(async()=>{
    const answer=await call('GET',base+'?zone=UTC');assert.equal(answer.status,200,answer.text);
    const row=answer.json.data.rows.find(r=>r.provider==='claude-code@work');
    return row?.days.some(d=>d.models.length)&&row;
  },'The Claude usage corpus was not indexed');
  assert.equal(row.label,'Work');
  assert.equal(row.identity,undefined);
  assert.deepEqual(row.days.map(({date,tokens,models})=>({date,tokens,models:models.map(({model,tokens})=>({model,tokens}))})),expected.days.map(({date,tokens,models})=>({date:shiftDay(date),tokens,models:models.map(({model,tokens})=>({model,tokens}))})));
  assert.deepEqual(Object.fromEntries(row.days.filter(d=>d.unknown).map(d=>[d.date,d.unknown])),Object.fromEntries(Object.entries(expected.unknown).map(([date,models])=>[shiftDay(date),models])));
  // The original scanner fixture used only its three small catalogs. The
  // resident merges them with its bundled catalog, whose exact Sonnet rate
  // wins over the fixture's fuzzy OpenRouter rate on both boxes.
  const costs=expected.days.map((d,i)=>d.models.map(m=>m.model==='claude-sonnet-5'?(i===0?0.018:0.00825):m.cost));
  assert.deepEqual(row.days.map(d=>d.models.map(m=>m.cost)),costs);
  assert.deepEqual(row.days.map(d=>d.cost),costs.map(day=>day.reduce((total,cost)=>total+cost,0)));
});

test('a Claude account keeps complete appended usage, replaces shorter logs and drops removed accounts',async t=>{
  const fs=await import('node:fs');
  const {computer}=await bench(t);
  const box=await computer('ClaudeUsageGrowth',undefined,isolated);
  const accounts=path.join(box.home,'agents','accounts.json');
  write(accounts,JSON.stringify([{formatVersion:1},{kind:'claude',id:'work',label:'Work'}]));
  const log=path.join(box.home,'agents','claude','accounts','work','projects','p','s.jsonl');
  const today=Math.floor(Date.now()/86400000)*86400000;
  const reply=(id,tokens,at=today-86400000+3600000)=>JSON.stringify({type:'assistant',timestamp:new Date(at).toISOString(),sessionId:'s',version:'2.1.300',requestId:'req_'+id,message:{id:'msg_'+id,model:'claude-opus-4-5',role:'assistant',usage:{input_tokens:tokens,output_tokens:0}}})+'\n';
  write(log,reply('one',1000)+reply('in',1,today-30*86400000+3600000)+reply('out',10,today-31*86400000+3600000));
  let resident=box.start(),call=box.connect(await resident.ready());
  const base='kuvo/agents/api/kuvo/usage';
  const read=async()=>{
    const answer=await call('GET',base+'?zone=UTC');assert.equal(answer.status,200,answer.text);
    return answer.json.data.rows.find(r=>r.provider==='claude-code@work');
  };
  const refresh=async()=>{const answer=await call('POST',base+'/refresh?zone=UTC',{});assert.equal(answer.status,200,answer.text);};
  const settle=async tokens=>until(async()=>{const row=await read();return row?.days[0]?.tokens===tokens&&row;},'Claude usage did not settle to '+tokens);
  await refresh();
  const initial=await settle(1000);
  assert.deepEqual(initial.days.map(d=>[d.date,d.tokens]),[[new Date(today-86400000).toISOString().slice(0,10),1000],[new Date(today-30*86400000).toISOString().slice(0,10),1]]);
  const partial=reply('three',7);
  fs.appendFileSync(log,reply('two',500)+partial.slice(0,40));await refresh();await settle(1500);
  fs.appendFileSync(log,partial.slice(40));await refresh();const grown=await settle(1507);
  await resident.stop();resident=box.start();call=box.connect(await resident.ready());
  assert.deepEqual((await settle(1507)).days,grown.days);
  write(log,reply('replacement',42));await refresh();const replaced=await settle(42);
  assert.equal(replaced.days.length,1);
  if(boxBinary().typescript){
    // The isolated reference resident has no account-mutation bridge. Seed
    // the same resulting roster on restart, leaving the historical log in
    // place so it cannot by itself create an active account row.
    await resident.stop();write(accounts,JSON.stringify([{formatVersion:1}]));
    resident=box.start();call=box.connect(await resident.ready());
  }else{
    const removed=await call('DELETE','kuvo/agents/api/kuvo/accounts/claude-code%40work');
    assert.equal(removed.status,200,removed.text);
  }
  await refresh();await until(async()=>!(await read()),'A removed account still appears in usage');
});

test('the shared Codex corpus keeps running-total deltas, forks, tiers and unknown model estimates',async t=>{
  const fs=await import('node:fs');
  const fixture=name=>new URL('../../box/test/fixtures/usage/'+name,import.meta.url);
  const expected=JSON.parse(fs.readFileSync(fixture('codex-expected.json'),'utf8'));
  const today=Math.floor(Date.now()/86400000)*86400000;
  const shift=today-86400000-Date.parse('2026-11-12T00:00:00Z');
  const dayOf=day=>new Date(day==='2026-10-02'?today-8*86400000:Date.parse(day+'T00:00:00Z')+shift).toISOString().slice(0,10);
  const {computer}=await bench(t);
  const box=await computer('CodexCorpus',undefined,isolated);
  const prices=path.join(box.home,'agents','usage','pricing');
  for(const [from,to] of [['pricing-codex-litellm.json','litellm.json'],['pricing-codex-models-dev.json','models_dev.json'],['pricing-codex-supplement.json','supplement.json']]){
    const data=JSON.parse(fs.readFileSync(fixture(from),'utf8'));
    if(to==='supplement.json')data.updated_at=new Date(today+86400000).toISOString();
    write(path.join(prices,to),JSON.stringify(data));
  }
  for(const name of expected.files){
    // Preserve the original JSONL byte shapes (including whitespace and
    // broken lines), changing only the timestamps in the fixture.
    const shifted=fs.readFileSync(fixture(name),'utf8').trimEnd().split('\n').map(line=>line
      .replace(/("timestamp"\s*:\s*")(\d{4}-\d{2}-\d{2})/g,(_,prefix,day)=>prefix+dayOf(day))
      .replace(/("started_at"\s*:\s*)(\d+)/g,(_,prefix,seconds)=>prefix+(Number(seconds)+shift/1000)));
    const parts=name.split('/').slice(1),file=parts.at(-1);
    const original=file.match(/rollout-(\d{4}-\d{2}-\d{2})/)[1];
    const day=dayOf(original);
    const shiftedFile=file.replace(original,day);
    const dest=parts[0]==='sessions'?['sessions',...day.split('-'),shiftedFile]:['archived_sessions',shiftedFile];
    write(path.join(box.env.CODEX_HOME,...dest),shifted.join('\n')+'\n');
  }
  let resident=box.start(),call=box.connect(await resident.ready());
  const base='kuvo/agents/api/kuvo/usage';
  assert.equal((await call('POST',base+'/refresh?zone=UTC',{})).status,200);
  const read=async()=>{
    const result=await call('GET',base+'?zone=UTC');assert.equal(result.status,200,result.text);return result.json.data;
  };
  const first=await until(async()=>{const data=await read();return data.totals.codex?.some(d=>d.models.length)&&data;},'Codex native usage corpus did not arrive');
  const shape=days=>days.map(({date,tokens,models})=>({date,tokens,models:models.map(({model,tokens})=>({model,tokens}))}));
  const check=(days,want)=>{
    assert.deepEqual(shape(days),shape(want.days.map(d=>({...d,date:dayOf(d.date)}))));
    for(const field of ['unknown','fallback'])assert.deepEqual(Object.fromEntries(days.filter(d=>d[field]).map(d=>[d.date,d[field]])),Object.fromEntries(Object.entries(want[field]).map(([date,models])=>[dayOf(date),models])));
    // The archive was moved inside the display window. After the original
    // Oct 6 price change auto-review is free; all other terms stay exact.
    const costs=want.days.map(d=>d.models.map(m=>d.date==='2026-10-02'&&m.model==='codex-auto-review'&&dayOf(d.date)>='2026-10-06'?0:m.cost));
    assert.deepEqual(days.map(d=>d.models.map(m=>m.cost)),costs);
    assert.deepEqual(days.map(d=>d.cost),costs.map(day=>day.reduce((total,cost)=>total+cost,0)));
  };
  check(first.totals.codex,expected.plain);
  assert.ok(first.pricing.codex.choices.includes(expected.fallback.model));
  const chosen=await call('PUT',base+'/pricing?zone=UTC',{codex:{fallback:expected.fallback.model}});
  assert.equal(chosen.status,200,chosen.text);
  check(chosen.json.data.totals.codex,expected.fallback);
  const denied=await call('PUT',base+'/pricing?zone=UTC',{codex:{fallback:'gpt-nope'}});
  assert.equal(denied.status,400,denied.text);
  await resident.stop();
  if(!boxBinary().typescript){
    // A previous Rust checkpoint might contain the rejected record. An old
    // parser generation must replace its derived rows, not skip an unchanged
    // file. Removing one priced row models stale derived data without editing
    // the underlying provider log.
    const {DatabaseSync}=await import('node:sqlite');
    const kept=new DatabaseSync(path.join(box.home,'agents','usage','insights.sqlite'));
    try{
      const source=kept.prepare("SELECT source FROM native_file WHERE kind='codex' ORDER BY source LIMIT 1").get().source;
      kept.prepare("UPDATE native_file SET cursor=json_remove(cursor,'$.codex_format') WHERE source=?").run(source);
      kept.prepare('DELETE FROM native_price WHERE source=?').run(source);
    }finally{kept.close();}
  }
  resident=box.start();call=box.connect(await resident.ready());
  const saved=await until(async()=>{const data=await read();return data.pricing.codex.fallback===expected.fallback.model&&JSON.stringify(data.totals.codex?.map(d=>d.tokens))===JSON.stringify(expected.fallback.days.map(d=>d.tokens))&&data;},'The fallback and its usage did not survive restart');
  check(saved.totals.codex,expected.fallback);
  const cleared=await call('PUT',base+'/pricing?zone=UTC',{codex:{fallback:null}});
  assert.equal(cleared.status,200,cleared.text);
  check(cleared.json.data.totals.codex,expected.plain);
});

test('one signed-in account adds distinct computers’ logs but never counts the same physical folder twice',{timeout:120000},async t=>{
  if(boxBinary().typescript)return t.skip('The scratch reference cannot confirm live Codex account identities.');
  const {computer,leases}=await bench(t);
  const studio=await computer('AccountStudio',undefined,isolated);
  const lab=await computer('AccountLab',undefined,isolated);
  const duplicate=await computer('AccountLabCopy',undefined,folder=>({...isolated(folder),CODEX_HOME:lab.env.CODEX_HOME}));
  const now=Date.now()-60000,reset=Math.floor(now/1000)+7200;
  const seed=(home,id,tokens,account='fixture-history-user')=>{
    const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:account,chatgpt_user_id:account+'-person'}})).toString('base64url'),'fixture'].join('.');
    write(path.join(home,'auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:account}}));
    const at=new Date(now).toISOString();
    write(path.join(home,'sessions','rollout-'+id+'.jsonl'),[
      {type:'session_meta',timestamp:at,payload:{id}},
      {type:'turn_context',payload:{model:'gpt-5'}},
      {type:'event_msg',timestamp:at,payload:{type:'token_count',info:{last_token_usage:{input_tokens:tokens,output_tokens:0,total_tokens:tokens}},rate_limits:{primary:{used_percent:20,window_minutes:300,resets_at:reset}}}}
    ].map(JSON.stringify).join('\n')+'\n');
  };
  seed(studio.env.CODEX_HOME,'studio',1000);seed(lab.env.CODEX_HOME,'lab',300);
  write(path.join(lab.home,'agents','accounts.json'),JSON.stringify([{formatVersion:1},{kind:'codex',id:'home',label:'Home'}]));
  seed(path.join(lab.home,'agents','codex','accounts','home'),'other',7,'fixture-other-user');
  const main=studio.connect(await studio.start().ready());
  const setup=await main('GET','kuvo/setup');
  const join=async box=>{
    const invite=await main('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});
    assert.equal(invite.status,201,invite.text);
    await until(()=>leases.leaseFor(setup.json.setup),'Studio did not claim the setup lease');
    const result=await box.join(invite.json.code);assert.equal(result.code,0,result.log);
    return box.connect(await box.start().ready());
  };
  const other=await join(lab),copy=await join(duplicate);
  const base='kuvo/agents/api/kuvo/usage';
  const limits='kuvo/agents/api/kuvo/limits';
  for(const call of [main,other,copy]){
    const answer=await call('GET',limits);assert.equal(answer.status,200,answer.text);
    assert.equal((await call('POST',base+'/refresh?zone=UTC',{})).status,200);
  }
  const read=async call=>{
    const response=await call('GET',base+'?zone=UTC');assert.equal(response.status,200,response.text);
    return response.json.data;
  };
  for(const call of [main,other,copy]){
    const data=await until(async()=>{
      const data=await read(call);
      return data.rows.find(r=>r.service==='codex'&&r.identity&&r.boxes.length===3)&&data.rows.find(r=>r.provider==='codex-cli@home'&&r.identity&&r.days[0]?.tokens===7)&&data;
    },'The shared Codex account did not converge',30000,250);
    const rows=data.rows.filter(r=>r.service==='codex'&&r.days.length>0);
    assert.equal(rows.length,2);
    const shared=rows.find(r=>r.boxes.length===3);
    const separate=rows.find(r=>r.provider==='codex-cli@home');
    assert.notEqual(shared.identity,separate.identity);
    assert.equal(shared.days[0].tokens,1300);
    assert.equal(shared.boxes.filter(b=>b.same).length,1);
    assert.equal(shared.boxes.filter(b=>b.here).length,1);
    assert.equal(separate.days[0].tokens,7);
    assert.equal(data.totals.codex[0].tokens,1307);
  }
});

test('Cursor exchanges only its own key, retries a refused access token once, and keeps a held request-based plan',async t=>{
  if(boxBinary().typescript)return t.skip('The isolated TS resident disables maker requests.');
  const http=await import('node:http');
  const fs=await import('node:fs');
  const now=Date.now(),day=86400000;
  let exchanges=0,fail=0,key='fixture-key',refuseKey=false;
  const access=()=>['e30',Buffer.from(JSON.stringify({sub:'auth0|fixture-cursor-user',exp:Math.floor(now/1000)+3600,sequence:exchanges})).toString('base64url'),'fixture'].join('.');
  const asked=[];
  const maker=http.createServer((req,res)=>{
    asked.push({path:req.url,auth:req.headers.authorization,cookie:req.headers.cookie});
    const send=(status,body,headers={})=>{res.writeHead(status,{'Content-Type':'application/json',...headers});res.end(JSON.stringify(body));};
    if(req.url==='/cursor/exchange'){assert.equal(req.headers.authorization,'Bearer '+key);exchanges++;if(refuseKey)send(401,{});else send(200,{accessToken:access()});return;}
    if(req.url==='/cursor/GetCurrentPeriodUsage'){
      if(fail){const code=fail;fail=0;send(code,{},code===429?{'Retry-After':'120'}:{});return;}
      assert.equal(req.headers.authorization,'Bearer '+access());send(200,{enabled:true,planUsage:{}});return;
    }
    if(req.url==='/cursor/GetPlanInfo'){send(200,{planInfo:{planName:'Enterprise'}});return;}
    if(req.url==='/cursor/usage-summary'){send(200,{billingCycleStart:new Date(now-day).toISOString(),billingCycleEnd:new Date(now+30*day).toISOString(),individualUsage:{onDemand:{limit:10000,used:2500}}});return;}
    if(req.url.startsWith('/cursor/usage?')){assert.equal(req.headers.cookie,'WorkosCursorSessionToken=fixture-cursor-user%3A%3A'+access());send(200,{'gpt-4':{numRequests:120,maxRequestUsage:500},startOfMonth:new Date(now-day).toISOString()});return;}
    if(req.url==='/cursor/GetSandUsageStatus'){send(200,{usagePercent:12,currentPeriodStart:new Date(now-day).toISOString(),nextResetTimestampUtc:new Date(now+6*day).toISOString()});return;}
    send(200,{});
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('CursorRenewal',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const auth=path.join(box.home,'agents','cursor','auth.json');
  write(auth,JSON.stringify({apiKey:'fixture-key'}));
  let resident=box.start(),call=box.connect(await resident.ready());
  const base='kuvo/agents/api/kuvo/limits';
  const refresh=async()=>{
    const response=await call('POST',base+'/refresh',{});assert.equal(response.status,200,response.text);
    return until(async()=>{const response=await call('GET',base);const data=response.json.data;return !data.refreshPending&&data.accounts.find(a=>a.provider==='cursor');},'Cursor plan did not settle');
  };
  const first=await refresh();
  assert.equal(first.plan,'Enterprise');
  assert.deepEqual(first.windows.map(w=>[w.id,w.amount]),[['total',{used:120,limit:500,unit:'requests'}],['on_demand',{used:25,limit:100,unit:'usd'}],['grok_bot',undefined]]);
  assert.equal(exchanges,1);
  assert.equal(JSON.stringify(first).includes('fixture-cursor-user'),false);
  assert.equal(JSON.stringify(first).includes('fixture-key'),false);
  const readings=windows=>windows.map(({read,...window})=>window);
  const same=await refresh();assert.deepEqual(readings(same.windows),readings(first.windows));assert.equal(exchanges,1);
  fail=401;const renewed=await refresh();assert.deepEqual(readings(renewed.windows),readings(first.windows));assert.equal(exchanges,2);
  fail=429;const held=await refresh();
  assert.equal(held.problem.kind,'rate-limited');assert.ok(held.problem.until>=now+120000);
  assert.deepEqual(held.windows,renewed.windows);
  const sent=asked.length;
  await resident.stop();resident=box.start(undefined,{KUVO_TEST_CURSOR_HISTORY:'1'});call=box.connect(await resident.ready());
  const stillHeld=await refresh();assert.equal(stillHeld.problem.kind,'rate-limited');assert.deepEqual(stillHeld.windows,renewed.windows);
  assert.equal((await call('POST','kuvo/agents/api/kuvo/usage/refresh?zone=UTC',{})).status,200);
  assert.equal(asked.length,sent,'A held maker must not be called again after restart');
  fs.unlinkSync(auth);
  const signedOut=await call('GET',base);assert.ok(!signedOut.json.data.accounts.some(a=>a.provider==='cursor'));
  key='fixture-denied';refuseKey=true;write(auth,JSON.stringify({apiKey:key}));
  const turnedAway=await refresh();assert.equal(turnedAway.problem.kind,'expired');assert.deepEqual(turnedAway.windows,[]);
  const deniedSent=asked.length;
  assert.equal((await refresh()).problem.kind,'expired');assert.equal(asked.length,deniedSent);
  await resident.stop();resident=box.start(undefined,{KUVO_TEST_CURSOR_HISTORY:'1'});call=box.connect(await resident.ready());
  assert.equal((await refresh()).problem.kind,'expired');
  assert.equal((await call('POST','kuvo/agents/api/kuvo/usage/refresh?zone=UTC',{})).status,200);
  assert.equal(asked.length,deniedSent,'A refused key must not be sent again across restart');
  await resident.stop();key='fixture-renewed';refuseKey=false;write(auth,JSON.stringify({apiKey:key}));
  resident=box.start();call=box.connect(await resident.ready());
  const replacement=await refresh();assert.equal(replacement.problem,undefined);assert.deepEqual(readings(replacement.windows),readings(first.windows));
});

test('Cursor plan meters match the shared subscription, team, credit and Grok Bot corpus',async t=>{
  if(boxBinary().typescript)return t.skip('The isolated TS resident disables Cursor provider reads.');
  const {readFileSync}=await import('node:fs');
  const http=await import('node:http');
  const corpus=JSON.parse(readFileSync(new URL('../../box/test/fixtures/usage/cursor-plan-cases.json',import.meta.url),'utf8')).cases;
  const now=Date.now(),day=86400000,shift=Math.floor(now/day)*day+30*day-1764892800000;
  const summaryShift=Math.floor(now/day)*day+30*day-Date.parse('2026-12-01T00:00:00Z');
  const shiftedSummary=raw=>{
    const summary=structuredClone(raw??{});
    for(const key of ['billingCycleStart','billingCycleEnd','startOfMonth'])if(typeof summary[key]==='string'&&Number.isFinite(Date.parse(summary[key])))summary[key]=new Date(Date.parse(summary[key])+summaryShift).toISOString();
    return summary;
  };
  let current,serial=0,bot={};
  const maker=http.createServer((req,res)=>{
    const args=current.args;
    const send=data=>{res.writeHead(200,{'Content-Type':'application/json'});res.end(JSON.stringify(data??{}));};
    if(req.url==='/cursor/exchange'){
      const access=['e30',Buffer.from(JSON.stringify({sub:'auth0|cursor-corpus-'+serial,exp:Math.floor(now/1000)+3600})).toString('base64url'),'fixture'].join('.');
      assert.equal(req.headers.authorization,'Bearer fixture-cursor-'+serial);send({accessToken:access});return;
    }
    if(req.url==='/cursor/GetCurrentPeriodUsage'){
      const usage=structuredClone(current.fn==='mapRequestBasedUsage'?{planUsage:{}}:args.usage??{planUsage:{}});
      for(const key of ['billingCycleStart','billingCycleEnd'])if(usage[key]!==undefined)usage[key]=Number(usage[key])+shift;
      send(usage);return;
    }
    if(req.url==='/cursor/GetPlanInfo'){send({planInfo:{planName:args.planName}});return;}
    if(req.url==='/cursor/GetCreditGrantsBalance'){send(args.creditGrants);return;}
    if(req.url==='/cursor/auth/stripe'){send({customerBalance:-(args.stripeBalanceCents??0)});return;}
    if(req.url==='/cursor/usage-summary'){send(shiftedSummary(args.summary));return;}
    if(req.url.startsWith('/cursor/usage?')){send(shiftedSummary(current.fn==='mapRequestBasedUsage'?args.usage:args.requestUsage));return;}
    if(req.url==='/cursor/GetSandUsageStatus'){send(bot);return;}
    send({});
  });
  await new Promise(resolve=>maker.listen(0,'127.0.0.1',resolve));
  t.after(()=>new Promise(resolve=>{maker.close(resolve);maker.closeAllConnections();}));
  const {computer}=await bench(t);
  const box=await computer('CursorPlanCorpus',undefined,folder=>({...isolated(folder),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:'+maker.address().port+'/'}));
  const auth=path.join(box.home,'agents','cursor','auth.json');
  const call=box.connect(await box.start().ready());
  const base='kuvo/agents/api/kuvo/limits';
  const refresh=async()=>{
    write(auth,JSON.stringify({apiKey:'fixture-cursor-'+ ++serial}));
    assert.equal((await call('POST',base+'/refresh',{})).status,200);
    return until(async()=>{const result=(await call('GET',base)).json.data;return !result.refreshPending&&result.accounts.find(a=>a.provider==='cursor'&&a.checked);},'Cursor corpus reading did not settle: '+current.name);
  };
  for(const each of corpus.filter(c=>c.fn==='mapUsage')){
    current=each;const reading=await refresh();const want=each.expected;
    if(want.error){
      assert.equal(reading.problem?.kind,each.args.usage.enabled===false?'ended':'failed',each.name);
      assert.deepEqual(reading.windows,[],each.name);continue;
    }
    assert.equal(reading.plan,want.plan,each.name);
    const windows=want.lines.filter(l=>l.used!==undefined);
    assert.deepEqual(reading.windows.map(w=>w.name),windows.map(w=>w.label),each.name);
    for(const [i,line] of windows.entries()){
      const w=reading.windows[i];assert.equal(w.used,line.used/line.limit,each.name+': '+line.label);
      assert.deepEqual(w.amount??null,line.format==='dollars'?{used:line.used,limit:line.limit,unit:'usd'}:null,each.name);
      assert.equal(w.resetsAt,line.resetsAt===null?undefined:line.resetsAt+shift,each.name);
      assert.equal(w.minutes,line.periodMs===null?undefined:line.periodMs/60000,each.name);
    }
    const credits=want.lines.find(l=>l.label==='Credits');assert.deepEqual(reading.balance??null,credits?{left:credits.dollars,unit:'usd'}:null,each.name);
    const spend=want.lines.find(l=>l.label==='On-demand'&&l.dollars!==undefined);
    assert.deepEqual(reading.extras??[],spend?[{id:'on_demand',name:'On-demand',kind:'spent',amount:spend.dollars,unit:'usd',resetsAt:Number(each.args.usage.billingCycleEnd)+shift}]:[],each.name);
  }
  current=corpus.find(c=>c.fn==='mapUsage'&&!c.expected.error);
  for(const each of corpus.filter(c=>c.fn==='grokBot')){
    bot=structuredClone(each.args.usage);
    const botShift=Math.floor(now/day)*day+7*day-Date.parse('2026-11-17T00:00:00Z');
    for(const key of ['currentPeriodStart','nextResetTimestampUtc'])if(bot[key])bot[key]=new Date(Date.parse(bot[key])+botShift).toISOString();
    const reading=await refresh();const w=reading.windows.find(w=>w.id==='grok_bot');const want=each.expected.line;
    if(!want){assert.equal(w,undefined,each.name);continue;}
    assert.equal(w.name,want.label,each.name);assert.equal(w.used,want.used/want.limit,each.name);
    assert.equal(w.minutes,want.periodMs/60000,each.name);assert.equal(w.resetsAt,want.resetsAt===null?undefined:want.resetsAt+botShift,each.name);
  }
  bot={};
  for(const each of corpus.filter(c=>c.fn==='mapSummary'||c.fn==='mapRequestBasedUsage')){
    current=each;const reading=await refresh();const want=each.expected;
    if(want.error){assert.equal(reading.problem?.kind,'failed',each.name);assert.deepEqual(reading.windows,[],each.name);continue;}
    assert.equal(reading.plan,want.plan,each.name);
    // The route intentionally states request usage once under Total usage.
    const unique=want.lines.filter(l=>l.used!==undefined&&!(l.label==='Requests'&&want.lines.some(o=>o.label==='Total usage')));
    const windows=unique.map(l=>({...l,label:l.label==='Requests'?'Total usage':l.label}));
    assert.deepEqual(reading.windows.map(w=>w.name),windows.map(w=>w.label),each.name);
    for(const [i,line] of windows.entries()){
      const w=reading.windows[i];assert.equal(w.used,line.used/line.limit,each.name);
      const unit=line.format==='dollars'?'usd':line.format==='count:requests'?'requests':null;
      assert.deepEqual(w.amount??null,unit?{used:line.used,limit:line.limit,unit}:null,each.name);
      assert.equal(w.resetsAt,line.resetsAt===null?undefined:line.resetsAt+summaryShift,each.name);
      assert.equal(w.minutes,line.periodMs===null?undefined:line.periodMs/60000,each.name);
    }
    const spend=want.lines.find(l=>l.label==='On-demand'&&l.dollars!==undefined);
    assert.deepEqual(reading.extras??[],spend?[{id:'on_demand',name:'On-demand',kind:'spent',amount:spend.dollars,unit:'usd'}]:[],each.name);
  }
});

test('a corrupt saved ring cannot panic the resident or invent fresh usage',async t=>{
  if(boxBinary().typescript)return t.skip('The accepted-reading cache is a Rust format.');
  const now=Date.now(),reset=Math.floor(now/1000)+7200;
  const {box,refresh,resident,seen}=await engineQuotaFixture(t,'CorruptRing',()=>({body:{plan_type:'plus',rate_limit:{primary_window:{used_percent:25,limit_window_seconds:18000,reset_at:reset}}}}),[engineSign('openai',{type:'oauth',access:'fixture-corrupt-ring',expires:now+3600000,metadata:{accountID:'fixture-account'}})]);
  assert.equal((await refresh()).rows[0].rings.short.used,0.25);
  await resident.stop();
  const fs=await import('node:fs');
  const file=path.join(box.home,'agents','usage','accepted-limits.json');
  const kept=JSON.parse(fs.readFileSync(file,'utf8'));
  kept.entries.openai.trails.primary.current.used=-0.25;
  write(file,JSON.stringify(kept));
  const call=box.connect(await box.start().ready());
  const reply=await call('GET','kuvo/agents/api/kuvo/limits');
  assert.equal(reply.status,200,reply.text);
  assert.ok(!reply.json.data.rows.find(r=>r.provider==='openai')?.rings.short,'Invalid kept evidence cannot look like a fresh 0% ring');
  assert.equal(seen.length,1,'Recovery did not call a maker on an idle startup');
});
