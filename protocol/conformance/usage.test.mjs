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
  return {read,sample,reset};
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

test('Insights indexes portable usage metadata only when first opened and does not double-count a reread (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('The demand-driven index is a Rust change.');
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
  assert.equal(existsSync(db),false,'an idle resident must not open the Insights index');
  const first=await call('GET','kuvo/agents/api/kuvo/insights?period=7&zone=UTC');
  assert.equal(first.status,200,first.text);
  assert.equal(first.json.data.totals.calls,1);
  assert.equal(first.json.data.totals.processed,652);
  assert.equal(first.json.data.totals.tokens,152);
  assert.equal(existsSync(db),true);
  const second=await call('GET','kuvo/agents/api/kuvo/insights?period=7&zone=UTC');
  assert.equal(second.status,200,second.text);
  assert.deepEqual(second.json.data.totals,first.json.data.totals);
});

test('a subscribed rail asks for a current reading without an idle polling loop (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('The demand-driven reader is Rust-only.');
  const codex=await scriptedCodex();
  t.after(()=>codex.close());
  const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:'fixture',chatgpt_user_id:'fixture-user'}})).toString('base64url'),'fixture'].join('.');
  write(path.join(codex.root,'.codex','auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:'fixture'}}));
  const reset=Math.floor(Date.now()/1000)+7200;
  write(path.join(codex.root,'codex-limits.json'),JSON.stringify({rateLimits:{primary:{usedPercent:31,windowDurationMins:300,resetsAt:reset}}}));
  const listener=await codex.start();
  const {openLine}=await import('./adapter.mjs');
  const line=await openLine(listener,'','kuvo/agents/api/kuvo/limits/events');
  try {
    assert.equal(line.status,200);
    for await(const frame of line.frames()){
      if(frame.changed&&codex.sent('account/rateLimits/read').length){
        const answer=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
        if(answer.json.data.rows.find(r=>r.provider==='codex-cli')?.rings.short?.used===0.31)break;
      }
    }
    assert.equal(codex.sent('account/rateLimits/read').length,1);
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
