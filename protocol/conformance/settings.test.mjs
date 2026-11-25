// The actual settings routes, with a private home and the same stored records on both boxes.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { bench } from "./bench.mjs";
import { write, openLine, boxBinary, until } from "./adapter.mjs";

import { sessionOf } from "./engine.mjs";

const row = (page, id) => page.groups.flatMap(g => g.rows).find(r => r.id === id);
const rev = fields => createHash("sha256").update(JSON.stringify(fields)).digest("base64url").slice(0,16);

test("settings lists sections and keeps the conversation threshold with its own revision", async t => {
  const { computer } = await bench(t);
  const box = await computer("Settings");
  const record = await box.start().ready();
  const ask = box.connect(record);
  const index = await ask("GET","kuvo/settings");
  assert.equal(index.status,200,index.text);
  assert.deepEqual(index.json.sections.map(s => s.id),["clock","conversations","permissions","browser","computer","git","worktrees","storage","archived","agents","vault","mcp","custom-auto"]);
  const shown = await ask("GET","kuvo/settings/conversations");
  assert.equal(shown.status,200,shown.text);
  const at = row(shown.json,"compaction");
  assert.equal(at.value,"400000");
  assert.equal(at.revision,rev({can:true,choices:["200000","400000","600000","never"],kind:"choice",value:"400000"}));
  const change = {row:at.id,was:at.revision,value:"600000"};
  const saved = await ask("PATCH","kuvo/settings/conversations",change);
  assert.equal(saved.status,200,saved.text);
  assert.equal(row(saved.json,at.id).value,"600000");
  assert.equal(readFileSync(path.join(box.home,"box","conversation-compaction.json"),"utf8"),'"600000"');
  const stale = await ask("PATCH","kuvo/settings/conversations",{...change,value:"never"});
  assert.equal(stale.status,409,stale.text);
  assert.deepEqual(stale.json,{message:"It changed since you read it.",page:saved.json});
  const invalid = await ask("PATCH","kuvo/settings/conversations",{row:at.id,was:row(saved.json,at.id).revision,value:"not a choice"});
  assert.equal(invalid.status,400,invalid.text);
  assert.equal(invalid.json.message,"Pick one of its choices.");
  const absent = await ask("GET","kuvo/settings/unknown");
  assert.equal(absent.status,404,absent.text);
  assert.equal(absent.json.message,"This computer has no settings by that name.");
});

test("Git settings retain independent revisions, clean text, and follow external edits", async t => {
  const { computer } = await bench(t);
  const box = await computer("Git");
  // The fixture is an added computer that owns its engine settings.
  write(path.join(box.home,"box","opencode.json"),'{"unrelated":true}');
  const record = await box.start().ready();
  const ask = box.connect(record);
  const shown = await ask("GET","kuvo/settings/git");
  assert.equal(shown.status,200,shown.text);
  const prefix = row(shown.json,"branch-prefix");
  const commit = row(shown.json,"commit-instructions");
  assert.equal(prefix.value,"kuvo/");
  assert.equal(prefix.revision,rev({can:true,kind:"text",value:"kuvo/"}));
  const changed = await ask("PATCH","kuvo/settings/git",{row:prefix.id,was:prefix.revision,value:" -/. feature / "});
  assert.equal(changed.status,200,changed.text);
  assert.equal(row(changed.json,prefix.id).value,"feature/");
  assert.equal(changed.json.configChanges.length,1);
  const receiptId = changed.json.configChanges[0];
  assert.match(receiptId,/^[a-f0-9]{32}$/);
  const receipts = JSON.parse(readFileSync(path.join(box.home,"box","config-apply","receipts.json"),"utf8"));
  const receipt = receipts.find(r=>r.id===receiptId);
  assert.deepEqual(receipt.changes,[{path:["branch_prefix"],before:null,after:"feature/"}]);
  const setup = await ask("GET","kuvo/setup");
  assert.equal(receipt.setup,setup.json.setup);
  assert.equal(receipt.epoch,setup.json.epoch);
  assert.equal(row(changed.json,commit.id).revision,commit.revision);
  const saved = await ask("PATCH","kuvo/settings/git",{row:commit.id,was:commit.revision,value:"  Keep it short.  "});
  assert.equal(saved.status,200,saved.text);
  assert.equal(row(saved.json,commit.id).value,"Keep it short.");
  const config = await ask("GET","kuvo/config");
  assert.deepEqual(config.json,{unrelated:true,branch_prefix:"feature/",commit_instructions:"Keep it short."});
  const stale = await ask("PATCH","kuvo/settings/git",{row:prefix.id,was:prefix.revision,value:""});
  assert.equal(stale.status,409,stale.text);
  const events = await openLine(record,"","kuvo/settings/git/events");
  try {
    const iter = events.frames();
    let next = await iter.next();
    assert.equal(row(next.value,"branch-prefix").value,"feature/");
    await ask("PATCH","kuvo/config",{changes:[{path:["branch_prefix"],value:""}]});
    next = await iter.next();
    assert.equal(row(next.value,"branch-prefix").value,"");
  } finally { events.close(); }
});

test("Skills checks a form without writing, creates the skill, and refuses a stale form", async t => {
  const { computer } = await bench(t);
  const box = await computer("Skills");
  write(path.join(box.home,"box","opencode.json"),'{}');
  const record = await box.start().ready();
  const ask = box.connect(record);
  const shown = await ask("GET","kuvo/settings/skills");
  assert.equal(shown.status,200,shown.text);
  const list = row(shown.json,"skills");
  assert.equal(list.kind,"list");
  const add = list.add;
  const route = "kuvo/settings/skills/skills/add";
  const values = {name:"Release Notes",description:"Write release notes",instructions:"# Notes"};
  const checked = await ask("POST",route+"/check",{was:add.form.revision,values});
  assert.equal(checked.status,200,checked.text);
  assert.equal(checked.json.ready,true);
  assert.deepEqual(checked.json.fields,[{id:"name",words:"It's saved as release-notes.",tone:"quiet"}]);
  const before = await ask("GET","kuvo/skills");
  assert.deepEqual(before.json.folders,[]);
  const created = await ask("POST",route,{was:add.form.revision,values});
  assert.equal(created.status,200,created.text);
  const skill = row(created.json,"skills").items.find(i=>i.id==="skill-release-notes");
  assert.equal(skill.title,"release-notes");
  const old = skill.actions.find(a=>a.id==="edit").form.revision;
  const edit = "kuvo/settings/skills/skill-release-notes/edit";
  const updated = await ask("POST",edit,{was:old,values:{description:"Summarize releases",instructions:"# New"}});
  assert.equal(updated.status,200,updated.text);
  const stale = await ask("POST",edit,{was:old,values:{description:"Stale",instructions:"Old"}});
  assert.equal(stale.status,409,stale.text);
  const kept = await ask("GET","kuvo/skills/release-notes");
  assert.equal(kept.json.text,'---\nname: "release-notes"\ndescription: "Summarize releases"\n---\n\n# New\n');
});

test("coding-agent automatic updates are off by default and the switch keeps the same file", async t => {
  const { computer } = await bench(t);
  const box = await computer("Computer");
  const ask = box.connect(await box.start().ready());
  const shown=await ask("GET","kuvo/settings/computer");
  assert.equal(shown.status,200,shown.text);
  const toggle=row(shown.json,"agents-update");
  assert.equal(toggle.value,false);
  const changed=await ask("PATCH","kuvo/settings/computer",{row:toggle.id,was:toggle.revision,value:true});
  assert.equal(changed.status,200,changed.text);
  assert.equal(row(changed.json,toggle.id).value,true);
  assert.equal(readFileSync(path.join(box.home,"box","agents-update-themselves.json"),"utf8"),"true");
});

test("Permissions keeps a rule without rewriting an unrelated rule, and refuses Full Access on", async t => {
  const { computer } = await bench(t);
  const box = await computer("Permissions");
  const prior = {action:"edit",resource:"notes.md",effect:"deny"};
  write(path.join(box.home,"box","opencode.json"),JSON.stringify({permissions:[prior]}));
  const ask = box.connect(await box.start().ready());
  const shown = await ask("GET","kuvo/settings/permissions");
  assert.equal(shown.status,200,shown.text);
  const full = row(shown.json,"full-access");
  assert.equal(full.value,false);
  const denied = await ask("PATCH","kuvo/settings/permissions",{row:full.id,was:full.revision,value:true});
  assert.equal(denied.status,403,denied.text);
  const edit = row(shown.json,"when-edit");
  const saved = await ask("PATCH","kuvo/settings/permissions",{row:edit.id,was:edit.revision,value:"deny"});
  assert.equal(saved.status,200,saved.text);
  assert.equal(row(saved.json,edit.id).value,"deny");
  assert.deepEqual((await ask("GET","kuvo/config")).json.permissions,[{action:"edit",resource:"*",effect:"deny"},prior]);
});

// The user's 0.8 scope changed after the TypeScript reference: Rust omits Auto everywhere.
test("a saved Auto session reads as Ask First without changing its saved mode", {skip:boxBinary().typescript}, async t => {
  const { computer } = await bench(t);
  const box = await computer("Auto postponed");
  write(path.join(box.home,"box","approval.json"),'{"last":"auto","fullAccess":false}');
  write(path.join(box.home,"box","opencode.json"),'{}');
  const session = {...sessionOf({id:"ses_oldAuto",directory:box.home}),metadata:{kuvo:{approval:"auto",applied:"auto"}},permissions:[]};
  box.engine.sessions.push(session);
  const ask = box.connect(await box.start().ready());
  const here=await ask("GET","kuvo/approval");
  assert.equal(here.status,200,here.text);
  assert.equal(here.json.default,"askFirst");
  assert.deepEqual(here.json.options.engine.map(o=>o.approval),["askFirst","acceptEdits","fullAccess"]);
  assert.deepEqual(here.json.options.agents.map(o=>o.approval),["askFirst","acceptEdits","fullAccess"]);
  const settings=await ask("GET","kuvo/settings/permissions");
  assert.equal(settings.status,200,settings.text);
  assert.equal(row(settings.json,"default-mode").value,"Ask First");
  assert.equal(settings.json.groups.some(g=>["sandbox","allowed"].includes(g.id)),false);
  assert.equal(settings.json.groups.flatMap(g=>g.rows).some(r=>r.id.startsWith("auto-held-")),false);
  const saved=await ask("GET","kuvo/sessions/ses_oldAuto/approval");
  assert.equal(saved.status,200,saved.text);
  assert.equal(saved.json.approval,"askFirst");
  assert.equal(saved.json.effective,"askFirst");
  assert.equal(saved.json.held,undefined);
  assert.equal(session.metadata.kuvo.approval,"auto");
  assert.equal(JSON.parse(readFileSync(path.join(box.home,"box","approval.json"),"utf8")).last,"auto");
  const created=await ask("POST","kuvo/sessions",{title:"New conversation",chat:true});
  assert.equal(created.status,200,created.text);
  const newSession=box.engine.sessions.find(s=>s.title==="New conversation");
  assert.equal(newSession.metadata.kuvo.approval,"askFirst");
  assert.equal(newSession.metadata.kuvo.applied,"askFirst");
  const sandbox=await ask("GET","kuvo/sandbox");
  assert.equal(sandbox.status,200,sandbox.text);
  assert.deepEqual(sandbox.json,{on:false,able:false,agents:false,reason:"windows",why:"Coming soon on Windows.",needsYou:true});
  assert.deepEqual((await ask("POST","kuvo/sandbox/check")).json,sandbox.json);
  const allowed=await ask("GET","kuvo/sandbox/allowed");
  assert.deepEqual(allowed.json,{allowed:[],owners:[{kind:"chats",name:"Chats"}]});
  assert.equal((await ask("DELETE","kuvo/sandbox/allowed/missing")).status,404);
});

test("MCP forms check and save a remote server, with independent timeout changes", async t => {
  const { computer } = await bench(t);
  const box = await computer("MCP");
  write(path.join(box.home,"box","opencode.json"),'{}');
  const ask=box.connect(await box.start().ready());
  const shown=await ask("GET","kuvo/settings/mcp");
  assert.equal(shown.status,200,shown.text);
  const list=row(shown.json,"servers");
  const route="kuvo/settings/mcp/servers/add";
  const values={name:"Example Server",runs:"remote",url:"mcp.example.test/mcp",oauth:false,headers:[],codemode:false};
  const check=await ask("POST",route+"/check",{was:list.add.form.revision,values});
  assert.equal(check.status,200,check.text);
  assert.equal(check.json.ready,true);
  assert.deepEqual(check.json.fields,[{id:"name",words:"It'll be called Example-Server.",tone:"quiet"},{id:"url",words:"It'll be saved as https://mcp.example.test/mcp.",tone:"quiet"}]);
  assert.deepEqual((await ask("GET","kuvo/config")).json,{});
  const added=await ask("POST",route,{was:list.add.form.revision,values});
  assert.equal(added.status,200,added.text);
  const server=row(added.json,"servers").items.find(i=>i.id==="server-Example-Server");
  assert.equal(server.value,true);
  assert.deepEqual((await ask("GET","kuvo/config")).json.mcp.servers["Example-Server"],{type:"remote",oauth:false,url:"https://mcp.example.test/mcp",codemode:false});
  const timeout=row(shown.json,"timeout-startup");
  const saved=await ask("PATCH","kuvo/settings/mcp",{row:timeout.id,was:timeout.revision,value:1234});
  assert.equal(saved.status,200,saved.text);
  assert.equal(row(saved.json,"timeout-startup").value,1234);
  const off=await ask("PATCH","kuvo/settings/mcp",{row:server.id,was:server.revision,value:false});
  assert.equal(off.status,200,off.text);
  assert.equal(row(off.json,"servers").items.find(i=>i.id===server.id).value,false);
});

test("Storage measures files by category and only the recorded outside worktree", async t => {
  const { computer } = await bench(t);
  const box = await computer("Storage");
  const outside=path.join(path.dirname(box.home),"projects");
  const tree=path.join(outside,"tree");
  write(path.join(box.home,"browser","data"),Buffer.alloc(150000,7));
  write(path.join(tree,"work"),Buffer.alloc(120000,9));
  write(path.join(outside,"mine","unrelated"),Buffer.alloc(3000000,3));
  write(path.join(box.home,"box","worktrees.json"),JSON.stringify({worktrees:[{root:tree,project:outside,branch:"topic"}]}));
  const ask=box.connect(await box.start().ready());
  const page=await until(async()=>{
    const reply=await ask("GET","kuvo/settings/storage");
    assert.equal(reply.status,200,reply.text);
    return row(reply.json,"used")?.parts && reply.json;
  },"Storage did not measure its files.");
  const used=row(page,"used");
  assert.deepEqual(used.parts.map(p=>p.id),["sessions","worktrees","made","browser","other"]);
  assert.ok(used.parts.find(p=>p.id==="browser").value>=150000);
  assert.ok(used.parts.find(p=>p.id==="worktrees").value>=120000);
  const folder=row(page,"worktree-folder-0");
  assert.equal(folder.title,"1 worktree in");
  assert.ok(folder.value>=120000 && folder.value<1000000);
  assert.equal(row(page,"manage-worktrees").section,"worktrees");
});

const allRows = page => page.groups.flatMap(g => g.rows.flatMap(r => [r,...(r.items ?? [])]));
const item = (page,id) => allRows(page).find(r=>r.id===id);
const action = (r,id) => [...(r.actions??[]),...(r.add?[r.add]:[])].find(a=>a.id===id);
const opening = (r,id) => Object.fromEntries(action(r,id).form.fields.map(f=>[f.id,f.value??""]));
const perform = (ask,section,r,id,values,headers,check=false) =>
  ask("POST",`kuvo/settings/${section}/${encodeURIComponent(r.id)}/${id}${check?"/check":""}`,
    {was:action(r,id).form?.revision??r.revision,...(values===undefined?{}:{values})},headers);

test("Git migration takes a legacy prefix and a Mac's preferences once", async t => {
  const {computer} = await bench(t);
  const box = await computer("Migration");
  write(path.join(box.home,"box/opencode.json"),'{"unrelated":true}');
  write(path.join(box.home,"box/git.json"),'{"branchPrefix":"legacy/","unknown":42}');
  const ask = box.connect(await box.start().ready());
  assert.equal(row((await ask("GET","kuvo/settings/git")).json,"branch-prefix").value,"legacy/");
  let answer=await ask("POST","kuvo/git/settings/move",{branchPrefix:"mac/",commitInstructions:"  Be brief.  "});
  assert.equal(answer.status,200,answer.text);
  assert.equal(answer.json.moved,true);
  assert.equal(row((await ask("GET","kuvo/settings/git")).json,"commit-instructions").value,"Be brief.");
  answer=await ask("POST","kuvo/git/settings/move",{pullRequestInstructions:"Not taken"});
  assert.equal(answer.status,200,answer.text);
  assert.equal(answer.json.moved,false);
  assert.equal(JSON.parse(readFileSync(path.join(box.home,"box/git.json"),"utf8")).branchPrefix,undefined);
  assert.equal(JSON.parse(readFileSync(path.join(box.home,"box/git.json"),"utf8")).unknown,42);
});

test("Agents keeps independent custom records and refuses invalid or stale forms", async t => {
  const {computer} = await bench(t);
  const box = await computer("Agents Settings");
  write(path.join(box.home,"box/opencode.json"),'{"agents":{"Reviewer":{"description":"Reviews changes.","mode":"subagent","unknown":42}}}');
  write(path.join(box.env.HOME,".claude","agents","fixture.md"),"---\nname: Fixture Reviewer\ndescription: Checks findings.\n---\nReview the findings.");
  const ask = box.connect(await box.start().ready());
  const page=await ask("GET","kuvo/settings/agents");
  assert.equal(page.status,200,page.text);
  const list=row(page.json,"agents");
  assert.ok(list.items.some(r=>r.id==="agent-build"));
  const values=opening(list,"add");
  const invalid=await perform(ask,"agents",list,"add",{...values,name:"Reviewer"},undefined,true);
  assert.equal(invalid.status,200,invalid.text);
  assert.equal(invalid.json.ready,false);
  const made=await perform(ask,"agents",list,"add",{...values,name:"Bug Hunter",description:"Finds bugs.",system:"Report the findings."});
  assert.equal(made.status,200,made.text);
  assert.ok(item(made.json,"agent-Bug Hunter"));
  const saved=JSON.parse(readFileSync(path.join(box.home,"box/opencode.json"),"utf8"));
  assert.equal(saved.agents["Bug Hunter"].description,"Finds bugs.");
  assert.equal(saved.agents.Reviewer.unknown,42);
  const imported=item(made.json,"claude-fixture-reviewer");
  assert.ok(imported);
  const brought=await perform(ask,"agents",imported,"import");
  assert.equal(brought.status,200,brought.text);
  const kept=JSON.parse(readFileSync(path.join(box.home,"box/opencode.json"),"utf8"));
  assert.deepEqual(kept.agents["fixture-reviewer"],{mode:"subagent",description:"Checks findings.",system:"Review the findings."});
  const original=item(made.json,"agent-Bug Hunter");
  const edit=await perform(ask,"agents",original,"edit",{...opening(original,"edit"),description:"Finds issues."});
  assert.equal(edit.status,200,edit.text);
  const stale=await perform(ask,"agents",original,"edit",{...opening(original,"edit"),description:"Stale"});
  assert.equal(stale.status,409,stale.text);
  const gone=await perform(ask,"agents",item(edit.json,original.id),"delete");
  assert.equal(gone.status,200,gone.text);
  assert.equal(item(gone.json,original.id),undefined);
});

test("Custom Auto keeps list revisions, form checks, order and limits in its own file", async t => {
  const {computer} = await bench(t);
  const box=await computer("Model Lists");
  const {startClaudeBridge}=await import("./claude.mjs");
  const bridge=await startClaudeBridge(boxBinary(),box.home,box.env,{engine:box.engine});
  t.after(async()=>{await bridge?.stop()});
  const ask=box.connect(await box.start().ready());
  const root="kuvo/agents/api/kuvo/auto";
  const answer=await ask("GET",root);
  assert.equal(answer.status,200,answer.text);
  const page=await ask("GET","kuvo/settings/custom-auto");
  assert.equal(page.status,200,page.text);
  const lists=row(page.json,"lists");
  const invalid=await perform(ask,"custom-auto",lists,"add",{name:42},undefined,true);
  assert.equal(invalid.status,200,invalid.text);
  assert.equal(invalid.json.ready,false);
  const added=await perform(ask,"custom-auto",lists,"add",{name:"Features"});
  assert.equal(added.status,200,added.text);
  const list=row(added.json,"lists").items.find(r=>r.title==="Features");
  assert.ok(list);
  const edited=await perform(ask,"custom-auto",list,"edit",{name:"Build"});
  assert.equal(edited.status,200,edited.text);
  const stale=await perform(ask,"custom-auto",list,"edit",{name:"Old"});
  assert.equal(stale.status,409,stale.text);
  const moved=await perform(ask,"custom-auto",item(edited.json,list.id),"up");
  assert.equal(moved.status,200,moved.text);
  assert.equal(row(moved.json,"lists").items[0].title,"Build");
  const persisted=JSON.parse(readFileSync(path.join(box.home,"agents/auto.json"),"utf8"));
  assert.equal(persisted.lists[0].name,"Build");
  assert.equal((await ask("DELETE",root+"/limits")).status,200);
});

test("Vault Settings forms never return values and edits keep the empty secret unchanged", async t => {
  const {computer}=await bench(t);
  const box=await computer("Vault Settings");
  const {randomBytes,generateKeyPairSync,sign}=await import("node:crypto");
  const signing=generateKeyPairSync("ed25519");
  const mac=randomBytes(32).toString("base64url");
  const id=randomBytes(12).toString("hex");
  const linkFile=path.join(box.home,"run/link.json");
  write(linkFile,JSON.stringify({ ...JSON.parse(readFileSync(linkFile,"utf8")), id, macs:[mac],
    macSigns:{[mac]:signing.publicKey.export({type:"spki",format:"der"}).subarray(-32).toString("base64url")} }));
  const ask=box.connect(await box.start().ready());
  const at=Date.now(),nonce=randomBytes(20).toString("base64url");
  const proof=sign(null,Buffer.from(`kuvo-vault-key\0${id}\0${at}\0${nonce}`),signing.privateKey).toString("base64url");
  assert.equal((await ask("POST","kuvo/vault/key",{mac,at,nonce,proof})).status,200);
  const header={"x-kuvo-from":"app"};
  const secret=randomBytes(24).toString("base64url");
  const page=await ask("GET","kuvo/settings/vault",undefined,header);
  assert.equal(page.status,200,page.text);
  const list=row(page.json,"items");
  const values={...opening(list,"add"),kind:"login",name:"Fixture login",site:"example.test",username:"tester",password:secret,signInOnly:true};
  const added=await perform(ask,"vault",list,"add",values,header);
  assert.equal(added.status,200,added.text);
  assert.ok(!added.text.includes(secret));
  const login=row(added.json,"items").items.find(i=>i.title==="Fixture login");
  assert.ok(login);
  const edit=opening(login,"edit");
  assert.equal(edit.password,"");
  const saved=await perform(ask,"vault",login,"edit",{...edit,username:"other"},header);
  assert.equal(saved.status,200,saved.text);
  assert.ok(!saved.text.includes(secret));
  const actual=await ask("GET","kuvo/vault",undefined,header);
  const record=actual.json.items.find(i=>i.name==="Fixture login");
  assert.equal(record.username,"other");
  assert.deepEqual((await ask("POST",`kuvo/vault/items/${record.id}/same`,{values:{password:secret}},header)).json,{same:true});
  const untrusted=await ask("GET","kuvo/settings/vault");
  assert.deepEqual(untrusted.json.groups,[]);
  assert.ok(!untrusted.text.includes(secret));
  // The signed request is bound to this exact outer route, not to a nested
  // vault read. Its authority is usable only during the admitted request.
  if (!boxBinary().typescript) {
    const pause=row(saved.json,"pause");
    const route="kuvo/settings/vault";
    const body={row:"pause",was:pause.revision,value:true};
    const at=Date.now(),nonce=randomBytes(20).toString("base64url");
    const hash=createHash("sha256").update(JSON.stringify(body)).digest("hex");
    const proof=sign(null,Buffer.from(`kuvo-vault-request\0${id}\0${at}\0${nonce}\0PATCH\0${route}\0${hash}`),signing.privateKey).toString("base64url");
    const signed={"x-kuvo-vault-proof":Buffer.from(JSON.stringify({mac,at,nonce,proof})).toString("base64url")};
    const paused=await ask("PATCH",route,body,signed);
    assert.equal(paused.status,200,paused.text);
    assert.equal(row(paused.json,"pause").value,true);
    assert.notEqual((await ask("PATCH",route,body,signed)).status,200);
  }
});
