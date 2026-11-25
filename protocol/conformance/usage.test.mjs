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
  const answered=[];
  const now=Date.now();
  const reset=Math.floor(now/1000)+7200;
  const replies={
    '/chatgpt':{plan_type:'plus',rate_limit:{primary_window:{used_percent:30,limit_window_seconds:18000,reset_at:reset}},credits:{has_credits:true,balance:'821.9'},rate_limit_reset_credits:{available_count:0}},
    '/grok':{config:{currentPeriod:{start:new Date(now-3*86400000).toISOString(),end:new Date(now+4*86400000).toISOString()},creditUsagePercent:64}},
    '/copilot':{login:'fixture',copilot_plan:'individual_pro',quota_reset_date:new Date(now+86400000).toISOString(),quota_snapshots:{premium_interactions:{entitlement:300,remaining:120,percent_remaining:40}}},
    '/openrouterKey':{data:{limit:null,is_free_tier:false}},
    '/openrouterCredits':{data:{total_credits:20,total_usage:7.5}},
    '/zai':{data:{limits:[{type:'TOKENS_LIMIT',unit:3,number:5,percentage:12,nextResetTime:reset*1000},{type:'TIME_LIMIT',currentValue:40,usage:100}]}},
    '/opencodeGo':{usage:{rolling:{percent:9,resetsAt:new Date(reset*1000).toISOString()},weekly:{percent:21,resetsAt:new Date(now+4*86400000).toISOString()}}}
  };
  const provider=http.createServer((req,res)=>{
    answered.push({url:req.url,auth:req.headers.authorization,account:req.headers['chatgpt-account-id']});
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
    sign('xai',{type:'oauth',access:'fixture-xai',expires:now+3600000}),
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
});

test('Claude Code answers its plan via its own demand-started control process (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('This fixture supplies the Rust native usage-control reader.');
  const {scriptedClaude}=await import('./claude.mjs');
  const claude=await scriptedClaude();
  t.after(()=>claude.close());
  write(path.join(claude.env.CLAUDE_CONFIG_DIR,'.claude.json'),JSON.stringify({oauthAccount:{accountUuid:'usage-fixture',organizationUuid:'fixture-org'}}));
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
  const row=(await refresh()).id;
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
  else if(request.method==='authenticate'){authed=request.params.methodId==='cached_token';answer({});}
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
  const call=box.connect(await box.start().ready());
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
  await until(()=>leases.leaseFor(setup.json.setup),'History Studio did not claim its relay lease');
  const invite=await main('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});
  assert.equal(invite.status,201,invite.text);
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
  await until(()=>leases.leaseFor(setup.json.setup),'Studio has no relay lease');
  const invite=await main('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});
  assert.equal(invite.status,201,invite.text);
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
  await until(()=>leases.leaseFor(setup.json.setup),'Holder did not claim a relay lease');
  const invite=await here('POST','kuvo/computers/add/code',{}, {'x-kuvo-from':'mac'});
  assert.equal(invite.status,201,invite.text);
  assert.equal((await asker.join(invite.json.code)).code,0);
  let resident=asker.start(),call=asker.connect(await resident.ready());
  const remote=async()=>{const r=(await call('GET',base)).json.data.rows.find(r=>r.id===row.id);return r?.extras?.find(e=>e.kind==='resets');};
  await until(async()=>{const extra=await remote();return extra?.on?.name==='ResetHolder'&&extra;},'Remote grant account did not arrive',30000,250);
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
  if(process.platform==='win32')return t.skip('The reference does not discover native Windows local Antigravity servers.');
  const fs=await import('node:fs');
  const https=await import('node:https');
  const {spawnSync}=await import('node:child_process');
  const {computer}=await bench(t);
  const at=Date.now(),methods=[];
  // Temporary self-signed fixture, never an installed trust root or a real key.
  const box=await computer('AntigravityOwnedServer',undefined,folder=>({...isolated(folder),KUVO_TEST_AG_PROC_ROOT:path.join(folder,'account','fixture-proc'),KUVO_TEST_USAGE_BASE:'http://127.0.0.1:1/'}));
  const key=path.join(box.env.HOME,'fixture-key.pem'),cert=path.join(box.env.HOME,'fixture-cert.pem');
  const made=spawnSync('openssl',['req','-x509','-newkey','rsa:2048','-nodes','-subj','/CN=fixture.invalid','-days','1','-keyout',key,'-out',cert],{stdio:'ignore'});
  assert.equal(made.status,0,'The fixture certificate could not be generated');
  const server=https.createServer({key:fs.readFileSync(key),cert:fs.readFileSync(cert)},(req,res)=>{
    const method=req.url.split('/').at(-1);methods.push(method);
    assert.equal(req.headers.authorization,undefined);
    assert.equal(req.headers['x-codeium-csrf-token'],'fixture-csrf');
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
  const call=box.connect(await box.start().ready());const base='kuvo/agents/api/kuvo/limits';
  assert.equal((await call('POST',base+'/refresh',{})).status,200);
  const row=await until(async()=>{const data=(await call('GET',base)).json.data;return !data.refreshPending&&data.rows.find(r=>r.provider==='antigravity'&&r.rings.short?.used===0.8);},'Own local server did not provide quota windows');
  assert.equal(row.plan,'Pro');assert.ok(methods.includes('RetrieveUserQuotaSummary'));assert.ok(methods.includes('GetUserStatus'));
});
