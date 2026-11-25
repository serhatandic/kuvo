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

test('a connected Helper menu requests fresh rings and the same accepted reading reaches its feed (Rust reader)',async t=>{
  if(boxBinary().typescript)return t.skip('This case supplies the Rust native demand reader.');
  const codex=await scriptedCodex();
  t.after(()=>codex.close());
  const jwt=['e30',Buffer.from(JSON.stringify({'https://api.openai.com/auth':{chatgpt_account_id:'fixture',chatgpt_user_id:'fixture-user'}})).toString('base64url'),'fixture'].join('.');
  write(path.join(codex.root,'.codex','auth.json'),JSON.stringify({tokens:{access_token:jwt,account_id:'fixture'}}));
  const reset=Math.floor(Date.now()/1000)+7200;
  write(path.join(codex.root,'codex-limits.json'),JSON.stringify({rateLimits:{primary:{usedPercent:43,windowDurationMins:300,resetsAt:reset}}}));
  const listener=await codex.start();
  assert.equal(codex.sent('account/rateLimits/read').length,0);
  const {openLine}=await import('./adapter.mjs');
  const line=await openLine(listener,'','kuvo/box/at-work/events');
  try {
    assert.equal(line.status,200);
    for await(const frame of line.frames()) {
      if(frame.rings?.find(r=>r.service==='codex')?.short?.used===0.43)break;
    }
    const now=await request(listener,'GET','kuvo/agents/api/kuvo/limits');
    assert.equal(now.json.data.rows.find(r=>r.provider==='codex-cli').rings.short.used,0.43);
    assert.equal(codex.sent('account/rateLimits/read').length,1);
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
