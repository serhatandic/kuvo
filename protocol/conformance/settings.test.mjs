// The actual settings routes, with a private home and the same stored records on both boxes.
import assert from "node:assert/strict";
import { createHash } from "node:crypto";
import { existsSync, mkdirSync, readFileSync, rmSync } from "node:fs";
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
  const features=(await ask("GET","kuvo/info")).json.features;
  for (const feature of ["settings","settings.vault","auto.lists"]) assert.ok(features.includes(feature));
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
    // The box must also follow a settings change made outside all its routes.
    write(path.join(box.home,"box","opencode.json"),JSON.stringify({unrelated:true,branch_prefix:"external/",commit_instructions:"Keep it short."}));
    next = await iter.next();
    assert.equal(row(next.value,"branch-prefix").value,"external/");
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
  box.engine.providers=[{id:"opencode"}];
  box.engine.models=[{providerID:"opencode",id:"fast",name:"Fast",variants:[{id:"high"}]},
    {providerID:"opencode",id:"small",name:"Small",variants:[]}];
  box.engine.integrations=[{id:"opencode",name:"OpenCode Zen",connections:[]},{id:"lab",name:"Lab",connections:[],methods:[{type:"oauth",id:"login"}]}];
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
  const id=list.id.slice("list-".length);
  const modelRow=row(moved.json,"models-"+id);
  const wrong={model:JSON.stringify(["opencode","small"]),variant:"high"};
  const checked=await perform(ask,"custom-auto",modelRow,"add-model",wrong,undefined,true);
  assert.equal(checked.status,200,checked.text);
  assert.equal(checked.json.ready,false);
  const badModel=await perform(ask,"custom-auto",modelRow,"add-model",wrong);
  assert.equal(badModel.status,400,badModel.text);
  assert.deepEqual(badModel.json.fields,checked.json.fields);
  const one=await perform(ask,"custom-auto",modelRow,"add-model",{model:JSON.stringify(["opencode","fast"]),variant:"high"});
  assert.equal(one.status,200,one.text);
  const two=await perform(ask,"custom-auto",row(one.json,"models-"+id),"add-model",{model:JSON.stringify(["opencode","small"]),variant:""});
  assert.equal(two.status,200,two.text);
  const reordered=await perform(ask,"custom-auto",item(two.json,"entry-"+id+"-1"),"up");
  assert.equal(reordered.status,200,reordered.text);
  assert.deepEqual(row(reordered.json,"models-"+id).items.map(r=>r.value.entry.modelID),["small","fast"]);
  if (!boxBinary().typescript) {
    const picker=await ask("GET","kuvo/models/offered");
    assert.ok(!picker.json.models.some(model=>model.providerID==="auto"));
  }
  const direct=await ask("GET",root);
  const revision=direct.json.data.lists.find(l=>l.id===id).revision;
  assert.equal((await ask("PUT",root+"/"+id,{revision,entries:[{providerID:"lab",modelID:"private"}]})).status,200);
  const unavailable=await ask("GET","kuvo/settings/custom-auto");
  assert.equal(item(unavailable.json,"entry-"+id+"-0").notice.words,"Not signed in: Custom Auto passes it by");
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
test("Archived Sessions publishes owner menus and checks a remote change again on its computer", {timeout:90000}, async t => {
  const {computer,leases}=await bench(t);
  const main = await computer("Studio");
  const peerBinary=process.env.KUVO_CONFORMANCE_SETTINGS_PEER_BIN?boxBinary(process.env.KUVO_CONFORMANCE_SETTINGS_PEER_BIN):boxBinary();
  const other = await computer("Lab",peerBinary);
  const nearRecord=await main.start().ready();
  const near = main.connect(nearRecord);
  const original=await near("GET","kuvo/setup");
  assert.equal(original.status,200,original.text);
  const invite=await near("POST","kuvo/computers/add/code",{},{"x-kuvo-from":"mac"});
  assert.equal(invite.status,201,invite.text);
  await until(()=>leases.leaseFor(original.json.setup),"The main computer did not claim a relay lease.");
  const joined=await other.join(invite.json.code);
  assert.equal(joined.code,0,joined.log);
  const session=sessionOf({id:"ses_archive_owner",title:"Archived on Lab"});
  session.metadata={"kuvo.archived":true};
  other.engine.sessions.push(session);
  const far = other.connect(await other.start().ready());
  const theirs=await far("GET","kuvo/setup");
  const peer=theirs.json.you.key;
  const remoteId=`session-${peer}:${session.id}`;
  const owner=await until(async()=>{
    const reply=await far("GET","kuvo/settings/archived");
    return item(reply.json,"session-"+session.id)&&reply.json;
  },"The owner must list the archived session.");
  const remote=await until(async()=>{
    const reply=await near("GET","kuvo/settings/archived");
    return item(reply.json,remoteId)&&reply.json;
  },"The archived menu must come from its owner.",20000);
  const copied=item(remote,remoteId);
  assert.deepEqual(copied.computer,{id:peer,name:"Lab"});
  assert.deepEqual(copied.actions,item(owner,"session-"+session.id).actions);
  assert.match(copied.line,/^[A-Z][a-z]{2} \d{1,2}, \d{4} at \d{1,2}:\d{2} [AP]M$/);
  assert.ok(item(remote,"delete-all-"+peer));
  const followed=await openLine(nearRecord,"","kuvo/settings/archived/events",{},45000);
  try {
    const frames=followed.frames();
    assert.ok(item((await frames.next()).value,remoteId));
    const restored=await perform(near,"archived",copied,"unarchive");
    assert.equal(restored.status,200,restored.text);
    let next;
    do { next=await frames.next(); assert.equal(next.done,false); } while(item(next.value,remoteId));
  } finally {followed.close();}
});

test("Agents and MCP read the running engine's catalog rather than inventing unavailable state", async t => {
  const {computer}=await bench(t);
  const box=await computer("Live Settings");
  write(path.join(box.home,"box/opencode.json"),JSON.stringify({mcp:{servers:{sample:{type:"local",command:["sample"]}}}}));
  box.engine.agents=[{id:"build",name:"Build",mode:"primary",description:"Builds things",permissions:[]}];
  box.engine.providers=[{id:"lab",name:"Lab"}];
  box.engine.models=[{id:"fast",providerID:"lab",name:"Fast"}];
  box.engine.mcpServers=[{name:"sample",status:{status:"connected"}}];
  const ask=box.connect(await box.start().ready());
  const agents=await ask("GET","kuvo/settings/agents");
  assert.equal(agents.status,200,agents.text);
  const model=action(row(agents.json,"agents"),"add").form.fields.find(f=>f.id==="model");
  assert.deepEqual(model.choices.map(c=>c.id),["","lab/fast"]);
  const mcp=await ask("GET","kuvo/settings/mcp");
  assert.equal(mcp.status,200,mcp.text);
  assert.ok(allRows(mcp.json).some(r=>r.title==="sample" && r.badge?.words==="Connected"),mcp.text);
  if (!boxBinary().typescript) {
    const catalogReads=()=>box.engine.asked.filter(r=>r.method==="GET"&&r.path==="/api/model").length;
    const initial=catalogReads();
    await ask("GET","kuvo/settings/agents");
    await ask("GET","kuvo/settings/custom-auto");
    assert.equal(catalogReads(),initial,"the pages share the existing model catalog cache");
  }
});

test("Permissions changes serialize per row, keep unrelated rows and follow the old route", async t => {
  const {computer}=await bench(t);
  const box=await computer("Permission Revisions");
  const otherRule={action:"read",resource:"example.txt",effect:"deny"};
  write(path.join(box.home,"box/opencode.json"),JSON.stringify({permissions:[otherRule]}));
  const record=await box.start().ready();
  const ask=box.connect(record);
  const first=(await ask("GET","kuvo/settings/permissions")).json;
  const shell=row(first,"when-shell"), other=row(first,"other-computers");
  const parallel=await Promise.all(["ask","deny"].map(value=>ask("PATCH","kuvo/settings/permissions",{row:shell.id,was:shell.revision,value})));
  assert.deepEqual(parallel.map(r=>r.status).sort(),[200,409]);
  const current=parallel.find(r=>r.status===200).json;
  assert.equal(row(current,other.id).revision,other.revision);
  const disabled=await ask("PATCH","kuvo/settings/permissions",{row:other.id,was:other.revision,value:false});
  assert.equal(disabled.status,200,disabled.text);
  assert.equal((await ask("GET","kuvo/approval")).json.otherComputers,false);
  const saved=row(disabled.json,shell.id);
  const reset=await ask("PATCH","kuvo/settings/permissions",{row:saved.id,was:saved.revision,value:"allow"});
  assert.equal(reset.status,200,reset.text);
  assert.deepEqual((await ask("GET","kuvo/config")).json.permissions,[otherRule]);
  const stream=await openLine(record,"","kuvo/settings/permissions/events");
  try {
    const iter=stream.frames();
    assert.equal(row((await iter.next()).value,other.id).value,false);
    assert.equal((await ask("PATCH","kuvo/approval",{otherComputers:true})).status,200);
    assert.equal(row((await iter.next()).value,other.id).value,true);
  } finally {stream.close();}
});

test("This Computer uses the added computer's update and device state", async t => {
  const {computer}=await bench(t);
  const box=await computer("Computer Settings");
  const ask=box.connect(await box.start().ready());
  const answer=await ask("GET","kuvo/settings/computer");
  assert.equal(answer.status,200,answer.text);
  const page=answer.json;
  assert.equal(row(page,"state").title,"Running");
  assert.equal(row(page,"devices").value,"On");
  assert.equal(row(page,"devices").kind,"status");
  assert.equal(page.groups.some(g=>g.id==="macos"),false);
  assert.ok(row(page,"version"));
});

test("reading catalog-backed Settings pages does not start an absent OpenCode", {skip:boxBinary().typescript}, async t => {
  const {computer}=await bench(t);
  const box=await computer("Cold Settings");
  write(path.join(box.home,"box/opencode.json"),'{}');
  const engineRecord=path.join(box.home,"run/engine.json");
  rmSync(engineRecord);
  const ask=box.connect(await box.start().ready());
  for (const section of ["agents","mcp","skills","custom-auto","computer"]) {
    const answer=await ask("GET","kuvo/settings/"+section);
    assert.equal(answer.status,200,answer.text);
  }
  assert.equal(existsSync(engineRecord),false);
  assert.equal(box.engine.asked.length,0);
});

// Settings › MCP Servers, with the servers a user keeps: one on this computer, one on the web, and
// Kuvo Code's own browser tools, which aren't listed.
const mcpServers = {files:{type:"local",command:["sh","-c","serve files"],environment:{ROOT:"/srv"},codemode:false},
  tracker:{type:"remote",url:"https://tracker.example.com/mcp"},"kuvo-browser":{type:"local",command:["kuvo-browser"]}};
const mcpHere = process.platform==="darwin"?"this Mac":"this computer";
const mcpBox = async (t,name,settings={mcp:{servers:mcpServers}}) => {
  const {computer}=await bench(t);
  const box=await computer(name);
  write(path.join(box.home,"box/opencode.json"),JSON.stringify(settings));
  const app={"x-kuvo-from":"app"};
  const record=await box.start().ready();
  const plain=box.connect(record);
  const ask=(method,route,body,headers=app)=>plain(method,route,body,headers);
  const page=async()=>{const r=await ask("GET","kuvo/settings/mcp");assert.equal(r.status,200,r.text);return r.json;};
  const config=()=>JSON.parse(readFileSync(path.join(box.home,"box/opencode.json"),"utf8"));
  const change=(r,value,was=r.revision)=>ask("PATCH","kuvo/settings/mcp",{row:r.id,value,was});
  const check=(r,id,values)=>perform(ask,"mcp",r,id,values,app,true);
  const submit=(r,id,values)=>perform(ask,"mcp",r,id,values,app);
  return {box,record,ask,page,config,change,check,submit};
};

test("MCP lists your servers with how each is doing, and Try Again connects a failed one", async t => {
  const {box,ask,page,submit}=await mcpBox(t,"MCP List");
  box.engine.mcpServers=[{name:"files",status:{status:"failed",error:"spawn serve ENOENT"}},{name:"tracker",status:{status:"needs_auth"}}];
  const listed=(await ask("GET","kuvo/settings")).json.sections.find(s=>s.id==="mcp");
  assert.deepEqual([listed.title,listed.group,listed.icon],["MCP Servers","Agents & Tools","sliders"]);
  const shown=await page();
  const servers=row(shown,"servers");
  assert.deepEqual(servers.items.map(i=>i.id),["server-files","server-tracker"]);
  assert.equal(shown.groups[0].title,"Servers");
  assert.equal(shown.groups[0].note,`A server on ${mcpHere} runs a command; one on the web is a URL.`);
  assert.deepEqual([servers.add.id,servers.add.title,servers.add.form.title,servers.add.form.submit],["add","Add Server…","Add Server","Add Server"]);
  const files=item(shown,"server-files");
  assert.deepEqual([files.kind,files.value,files.can,files.line],["switch",true,true,'sh -c "serve files"']);
  assert.deepEqual(files.badge,{words:"Couldn’t connect",tone:"failed"});
  assert.deepEqual(files.notice,{words:"spawn serve ENOENT",tone:"warning"});
  assert.deepEqual(files.actions.map(a=>a.title),["Try Again","Edit…","Remove…"]);
  assert.deepEqual([files.actions[2].style,files.actions[2].confirm],["destructive",{title:"Remove files?",body:"The agent loses its tools.",button:"Remove"}]);
  const tracker=item(shown,"server-tracker");
  assert.equal(tracker.line,"https://tracker.example.com/mcp");
  assert.deepEqual(tracker.badge,{words:"Needs sign-in",tone:"warning"});
  const startup=row(shown,"timeout-startup");
  assert.deepEqual([startup.title,startup.kind,startup.placeholder,startup.can],["Time to start, in milliseconds","number","30000",true]);
  assert.deepEqual([shown.groups.at(-1).title,shown.groups.at(-1).note],["Timeouts","For every server, unless one sets its own."]);
  assert.equal(shown.notes,undefined);
  // Try Again asks the engine to connect it again.
  const tried=await submit(files,"try-again");
  assert.equal(tried.status,200,tried.text);
  assert.ok(box.engine.asked.some(r=>r.method==="POST"&&r.path==="/api/experimental/mcp/files/connect"));
  // How a server is doing never makes a reading stale.
  box.engine.mcpServers[0].status={status:"connected"};
  const now=item(await page(),"server-files");
  assert.deepEqual(now.badge,{words:"Connected",tone:"good"});
  assert.equal(now.notice,undefined);
  assert.deepEqual(now.actions.map(a=>a.id),["edit","remove"]);
  assert.equal(now.revision,files.revision);
  // With its engine down, the servers are still listed, and the page says why it can't say how they are.
  await box.engine.close();
  const down=await page();
  assert.equal(item(down,"server-files").badge,undefined);
  assert.deepEqual(down.notes,[{words:`The engine isn’t running on ${mcpHere}, so how each server is doing isn’t known. They’re listed as the settings have them.`,tone:"warning"}]);
});

test("MCP switches a server off and on, refusing a stale switch with the page now", async t => {
  const {page,config,change}=await mcpBox(t,"MCP Switch");
  const files=item(await page(),"server-files");
  const off=await change(files,false);
  assert.equal(off.status,200,off.text);
  assert.equal(config().mcp.servers.files.disabled,true);
  assert.deepEqual([item(off.json,"server-files").value,item(off.json,"server-files").badge],[false,{words:"Off",tone:"quiet"}]);
  const stale=await change(files,false);
  assert.equal(stale.status,409,stale.text);
  assert.equal(stale.json.message,"It changed since you read it.");
  assert.equal(item(stale.json.page,"server-files").value,false);
  assert.equal((await change(item(await page(),"server-tracker"),false)).status,200);
  assert.equal((await change(item(await page(),"server-files"),"off")).status,400);
  assert.equal((await change(item(await page(),"server-files"),true)).status,200);
  assert.equal(config().mcp.servers.files.disabled,undefined);
  assert.equal(config().mcp.servers.tracker.disabled,true);
});

test("MCP Add Server checks as it's typed, fills in a pasted setup, refuses without writing, and adds", async t => {
  const {box,page,config,change,check,submit}=await mcpBox(t,"MCP Add");
  const servers=row(await page(),"servers");
  assert.deepEqual(servers.add.form.fields.map(f=>[f.id,f.kind,f.shown?.is??null]),[
    ["name","text",null],["runs","choice",null],["command","lines",["local"]],["cwd","folder",["local"]],
    ["environment","pairs",["local"]],["url","text",["remote"]],["oauth","switch",["remote"]],["headers","pairs",["remote"]],["codemode","switch",null]]);
  assert.deepEqual(servers.add.form.fields[1].choices,[{id:"local",title:`On ${mcpHere}`},{id:"remote",title:"On the web"}]);
  const opened=opening(servers,"add");
  let checked=(await check(servers,"add",{...opened,name:"my files"})).json;
  assert.equal(checked.ready,false);
  assert.ok(checked.fields.some(f=>f.id==="name"&&f.words==="It'll be called my-files."&&f.tone==="quiet"),JSON.stringify(checked));
  checked=(await check(servers,"add",{...opened,name:"files",command:"sh -c 'x'"})).json;
  assert.equal(checked.ready,false);
  assert.ok(checked.fields.some(f=>f.id==="name"&&f.words==="There's a server called files already."));
  assert.ok(checked.fields.some(f=>f.id==="command"&&f.words===`sh is on ${mcpHere}.`&&f.tone==="good"),JSON.stringify(checked));
  checked=(await check(servers,"add",{...opened,name:"kuvo-browser",command:"no-such-program-here --flag"})).json;
  assert.ok(checked.fields.some(f=>f.id==="name"&&f.words==="Kuvo Code uses that name for its own tools."));
  assert.ok(checked.fields.some(f=>f.id==="command"&&f.words===`no-such-program-here isn’t on ${mcpHere}. Install it there first, or use a command it has.`),JSON.stringify(checked));
  checked=(await check(servers,"add",{...opened,name:"web",runs:"remote",url:"not an address"})).json;
  assert.ok(checked.fields.some(f=>f.id==="url"&&f.words==="That isn’t a web address. Give one like https://example.com/mcp."&&f.tone==="warning"));
  // A server's setup pasted into its name fills the form in.
  const pasted=JSON.stringify({mcpServers:{github:{command:"npx",args:["-y","@github/mcp"],env:{GITHUB_TOKEN:"x"}}}});
  checked=(await check(servers,"add",{...opened,name:pasted})).json;
  assert.deepEqual(checked.values,{name:"github",runs:"local",command:"npx -y @github/mcp",environment:[{key:"GITHUB_TOKEN",value:"x"}]});
  assert.ok(checked.fields.some(f=>f.id==="command"&&f.words==="Filled in from the setup you pasted."&&f.tone==="good"));
  const remote=(await check(servers,"add",{...opened,name:"claude mcp add --transport http linear https://mcp.linear.app/mcp"})).json;
  assert.deepEqual([remote.values.name,remote.values.runs,remote.values.url],["linear","remote","https://mcp.linear.app/mcp"]);
  const several=(await check(servers,"add",{...opened,name:JSON.stringify({mcpServers:{a:{command:"a"},b:{command:"b"}}})})).json;
  assert.ok(several.fields.some(f=>f.id==="name"&&f.words==="This sets up 2 servers (a, b). Paste one server’s part at a time."&&f.tone==="warning"));
  // What isn't ready is refused with the same words, and nothing is written.
  const before=readFileSync(path.join(box.home,"box/opencode.json"),"utf8");
  const refused=await submit(servers,"add",{...opened,name:"files",command:"sh"});
  assert.equal(refused.status,400,refused.text);
  assert.equal(refused.json.message,"There's a server called files already.");
  assert.ok(refused.json.fields.some(f=>f.id==="name"&&f.words==="There's a server called files already."&&f.tone==="warning"));
  assert.equal(readFileSync(path.join(box.home,"box/opencode.json"),"utf8"),before);
  assert.equal((await submit(servers,"add")).status,400);
  // Another server changed meanwhile: the form read before still goes.
  await change(item(await page(),"server-tracker"),false);
  const added=await submit(servers,"add",{...opened,name:"docs",command:"npx -y docs-server ~/Docs",environment:[{key:"TOKEN",value:"{env:DOCS}"}],codemode:true});
  assert.equal(added.status,200,added.text);
  assert.deepEqual(config().mcp.servers.docs,{type:"local",command:["npx","-y","docs-server","~/Docs"],environment:{TOKEN:"{env:DOCS}"},codemode:true});
  assert.deepEqual([item(added.json,"server-docs").value,item(added.json,"server-docs").line],[true,"npx -y docs-server ~/Docs"]);
});

test("MCP Edit keeps what its form doesn't show and goes stale, Remove keeps the rest, and the page follows the file", async t => {
  const {box,record,page,config,submit}=await mcpBox(t,"MCP Edit",{mcp:{servers:{files:{type:"local",command:["sh"],timeout:9000,codemode:true},tracker:mcpServers.tracker,"kuvo-browser":mcpServers["kuvo-browser"]}}});
  const files=item(await page(),"server-files");
  const edit=action(files,"edit");
  assert.deepEqual([edit.form.title,edit.form.submit],["files","Save"]);
  const values=opening(files,"edit");
  assert.deepEqual([values.runs,values.command,values.codemode],["local","sh",true]);
  const saved=await submit(files,"edit",{...values,runs:"remote",url:"mcp.example.com/mcp",oauth:false});
  assert.equal(saved.status,200,saved.text);
  assert.deepEqual(config().mcp.servers.files,{type:"remote",url:"https://mcp.example.com/mcp",oauth:false,timeout:9000,codemode:true});
  const stale=await submit(files,"edit",{...values,command:"sh -x"});
  assert.equal(stale.status,409,stale.text);
  assert.equal(config().mcp.servers.files.type,"remote");
  const removed=await submit(item(await page(),"server-files"),"remove");
  assert.equal(removed.status,200,removed.text);
  assert.deepEqual(Object.keys(config().mcp.servers).sort(),["kuvo-browser","tracker"]);
  assert.equal(item(removed.json,"server-files"),undefined);
  const lines=await openLine(record,"","kuvo/settings/mcp/events");
  try {
    const frames=lines.frames();
    assert.ok(item((await frames.next()).value,"server-tracker"));
    write(path.join(box.home,"box/opencode.json"),JSON.stringify({mcp:{servers:{other:{type:"local",command:["sh"]}}}}));
    let next;
    do { next=await frames.next(); assert.equal(next.done,false); } while(!item(next.value,"server-other"));
    assert.equal(item(next.value,"server-tracker"),undefined);
  } finally { lines.close(); }
});

test("MCP adds a popular server from its row, and keeps a timeout as a number unless set by hand otherwise", async t => {
  const {box,page,config,change,submit}=await mcpBox(t,"MCP Popular");
  write(path.join(box.home,"bin/mcp-catalog.json"),JSON.stringify({servers:[{id:"context7",title:"Context7",summary:"Docs for libraries.",access:"open",settings:{type:"remote",url:"https://mcp.context7.com/mcp"}}]}));
  const shown=await page();
  const open=shown.groups.find(g=>g.id==="popular-open");
  assert.equal(open.title,"No Account Needed");
  assert.match(open.note,/official MCP Registry/);
  const added=await submit(item(shown,"popular-context7"),"add");
  assert.equal(added.status,200,added.text);
  assert.deepEqual(config().mcp.servers.context7,{codemode:false,type:"remote",url:"https://mcp.context7.com/mcp"});
  assert.deepEqual([item(added.json,"popular-context7").kind,item(added.json,"popular-context7").value],["status","Added"]);
  assert.ok(item(added.json,"server-context7"));
  const startup=row(await page(),"timeout-startup");
  assert.equal((await change(startup,45000)).status,200);
  assert.equal(config().mcp.timeout.startup,45000);
  write(path.join(box.home,"box/opencode.json"),JSON.stringify({mcp:{timeout:{startup:"slow"}}}));
  const handwritten=row(await page(),"timeout-startup");
  assert.deepEqual(handwritten.can,{why:"This is set in the settings file as something other than a number. Change it there."});
  const refused=await change(handwritten,1000);
  assert.equal(refused.status,403,refused.text);
  assert.equal(config().mcp.timeout.startup,"slow");
});

// Settings › Agents, with the engine's built-in agents and one of yours.
const agentBuiltIns=[{id:"build",name:"Build",mode:"primary"},{id:"plan",name:"Plan",mode:"primary"},{id:"general",name:"General",mode:"subagent"},
  {id:"explore",name:"Explore",mode:"subagent"},{id:"title",name:"Title",mode:"primary",hidden:true}];
const agentSettings={default_agent:"reviewer",agents:{
  reviewer:{mode:"all",description:"Reviews changes\nand says what's wrong",steps:12,permissions:[{action:"edit",resource:"*",effect:"deny"},{action:"shell",resource:"git diff *",effect:"allow"}]},
  plan:{system:"Plan in short steps."},title:{model:"lab/fast"}}};
const agentsBox=async(t,name,settings=agentSettings,{app=true}={})=>{
  const {computer}=await bench(t);
  const box=await computer(name);
  write(path.join(box.home,"box/opencode.json"),JSON.stringify(settings));
  box.engine.agents=[...agentBuiltIns,{id:"reviewer",name:"reviewer",mode:"subagent"}];
  box.engine.providers=[{id:"lab"}];
  box.engine.models=[{id:"fast",providerID:"lab",name:"Lab Fast"},{id:"big",providerID:"lab",name:"Lab Big"},{id:"off",providerID:"elsewhere",name:"No Account"}];
  const headers=app?{"x-kuvo-from":"app"}:{};
  const plain=box.connect(await box.start().ready());
  const ask=(method,route,body)=>plain(method,route,body,headers);
  const page=async()=>{const r=await ask("GET","kuvo/settings/agents");assert.equal(r.status,200,r.text);return r.json;};
  const config=()=>JSON.parse(readFileSync(path.join(box.home,"box/opencode.json"),"utf8"));
  const change=(r,value,was=r.revision)=>ask("PATCH","kuvo/settings/agents",{row:r.id,value,was});
  const check=(r,id,values)=>perform(ask,"agents",r,id,values,headers,true);
  const submit=(r,id,values)=>perform(ask,"agents",r,id,values,headers);
  const field=(r,id,f)=>action(r,id).form.fields.find(x=>x.id===f);
  return {box,ask,page,config,change,check,submit,field};
};

test("Agents lists the built-in agents, then yours, each with how it's used, and keeps listing them with the engine down", async t => {
  const {box,ask,page,field}=await agentsBox(t,"Agents List");
  const listed=(await ask("GET","kuvo/settings")).json.sections.find(s=>s.id==="agents");
  assert.deepEqual([listed.title,listed.group,listed.icon],["Agents","Agents & Tools","bot"]);
  const shown=await page();
  const agents=row(shown,"agents");
  assert.deepEqual(agents.items.map(i=>i.title),["Build","Plan","General","Explore","reviewer"]);
  assert.deepEqual([agents.add.id,agents.add.title,agents.add.form.title,agents.add.form.submit],["add","New Agent…","New Agent","Create Agent"]);
  const build=item(shown,"agent-build");
  assert.deepEqual([build.kind,build.value,build.can,build.line],["switch",true,true,"A mode · The default agent: it can change files and run commands."]);
  assert.deepEqual(build.actions.map(a=>a.title),["Edit…"]);
  const plan=item(shown,"agent-plan");
  assert.deepEqual(plan.badge,{words:"Changed",tone:"quiet"});
  assert.deepEqual(plan.actions.map(a=>a.title),["Edit…","Reset to Built-In…"]);
  assert.deepEqual([plan.actions[1].style,plan.actions[1].confirm.title,plan.actions[1].confirm.button],["destructive","Reset Plan to how it's built in?","Reset"]);
  const reviewer=item(shown,"agent-reviewer");
  assert.equal(reviewer.line,"A mode and a subagent · Reviews changes");
  assert.deepEqual([reviewer.actions[1].id,reviewer.actions[1].title,reviewer.actions[1].confirm],["delete","Delete Agent…",
    {title:"Delete reviewer?",body:`It's removed from ${process.platform==="darwin"?"this Mac":"this computer"}'s settings.`,button:"Delete"}]);
  assert.equal(shown.notes,undefined);
  await box.engine.close();
  const down=await page();
  assert.deepEqual(row(down,"agents").items.map(i=>i.id),["agent-build","agent-plan","agent-general","agent-explore","agent-reviewer"]);
  assert.equal(down.notes[0].tone,"warning");
  assert.match(down.notes[0].words,/isn’t answering/);
  assert.deepEqual(field(item(down,"agent-reviewer"),"edit","model").choices.map(c=>c.id),[""]);
});

test("Agents turns one off and on through the engine's settings, and one turned off stops being where sessions start", async t => {
  const {page,config,change}=await agentsBox(t,"Agents Switch");
  let shown=(await change(item(await page(),"agent-reviewer"),false)).json;
  assert.equal(config().agents.reviewer.disabled,true);
  assert.equal(config().default_agent,undefined);
  assert.equal(item(shown,"agent-reviewer").value,false);
  assert.equal((await change(item(await page(),"agent-reviewer"),false,"old")).status,409);
  shown=(await change(item(shown,"agent-reviewer"),true)).json;
  assert.equal(config().agents.reviewer.disabled,undefined);
  assert.equal((await change(item(await page(),"agent-explore"),false)).status,200);
  assert.deepEqual(config().agents.explore,{disabled:true});
  assert.equal((await change(item(await page(),"agent-explore"),true)).status,200);
  assert.equal(config().agents.explore,undefined);
});

test("Agents New Agent checks as it's typed, refuses with the same words, and makes an agent the engine reads", async t => {
  const {box,page,config,check,submit,field}=await agentsBox(t,"Agents New");
  const agents=row(await page(),"agents");
  const opened=opening(agents,"add");
  assert.deepEqual(action(agents,"add").form.fields.map(f=>[f.id,f.kind]),[
    ["name","text"],["mode","choice"],["description","lines"],["model","choice"],["system","lines"],
    ["may-edit","choice"],["may-shell","choice"],["may-read","choice"],["may-external_directory","choice"],
    ["may-webfetch","choice"],["may-websearch","choice"],["may-subagent","choice"],["may-skill","choice"],
    ["allowed","lines"],["asks","lines"],["never","lines"]]);
  assert.equal(opened.mode,"subagent");
  assert.deepEqual(field(agents,"add","model").choices.map(c=>c.id),["","lab/fast","lab/big"]);
  const external=field(agents,"add","may-external_directory");
  assert.deepEqual([external.value,external.choices],["ask",[{id:"allow",title:"Allow"},{id:"ask",title:"Ask First (Default)"},{id:"deny",title:"Don't Allow"}]]);
  const has=(c,id,words,tone)=>c.fields.some(f=>f.id===id&&(words instanceof RegExp?words.test(f.words):f.words===words)&&(!tone||f.tone===tone));
  let checked=(await check(agents,"add",{...opened,name:"Bug  Hunter!"})).json;
  assert.equal(checked.ready,true,JSON.stringify(checked));
  assert.ok(has(checked,"name","It'll be called Bug Hunter.","quiet"),JSON.stringify(checked));
  assert.ok(has(checked,"mode",/another agent hands it part of the work/,"quiet"));
  checked=(await check(agents,"add",{...opened,name:"Reviewer"})).json;
  assert.equal(checked.ready,false);
  assert.ok(has(checked,"name","There's an agent called Reviewer already.","warning"));
  assert.equal((await check(agents,"add",{...opened,name:"plan"})).json.ready,false);
  checked=(await check(agents,"add",{...opened,name:"x",asks:"git push *"})).json;
  assert.ok(has(checked,"asks","“git push *” needs what it's for first, like Run commands: npm test *.","warning"),JSON.stringify(checked));
  checked=(await check(agents,"add",{...opened,name:"x",never:"Run commands: *"})).json;
  assert.ok(has(checked,"never","For all of them, set Run commands above.","warning"),JSON.stringify(checked));
  for (const name of ["__proto__","toString","x".repeat(65)]) assert.equal((await check(agents,"add",{...opened,name})).json.ready,false,name);
  const before=readFileSync(path.join(box.home,"box/opencode.json"),"utf8");
  const refused=await submit(agents,"add",{...opened,name:"reviewer"});
  assert.equal(refused.status,400,refused.text);
  assert.equal(refused.json.message,"There's an agent called reviewer already.");
  assert.equal((await submit(agents,"add",{...opened,name:"__proto__"})).status,400);
  assert.equal(readFileSync(path.join(box.home,"box/opencode.json"),"utf8"),before);
  const made=await submit(agents,"add",{...opened,name:"Bug Hunter",description:"Finds bugs.",model:"lab/big",system:"Look for bugs; change nothing.",
    "may-edit":"deny",allowed:"Run commands: npm test *\nshell: bun test *",never:"open web pages: https://*.internal/*"});
  assert.equal(made.status,200,made.text);
  assert.deepEqual(config().agents["Bug Hunter"],{mode:"subagent",description:"Finds bugs.",model:"lab/big",system:"Look for bugs; change nothing.",
    permissions:[{action:"edit",resource:"*",effect:"deny"},{action:"shell",resource:"npm test *",effect:"allow"},
      {action:"shell",resource:"bun test *",effect:"allow"},{action:"webfetch",resource:"https://*.internal/*",effect:"deny"}]});
  const hunter=item(made.json,"agent-Bug Hunter");
  assert.deepEqual([hunter.title,hunter.line],["Bug Hunter","A subagent · Finds bugs."]);
  const edit=opening(hunter,"edit");
  assert.deepEqual([edit["may-edit"],edit.allowed,edit.never,edit.on],["deny","Run commands: npm test *\nRun commands: bun test *","Open web pages: https://*.internal/*",true]);
});

test("Agents Edit saves what changed, keeps what it doesn't show, writes nothing for what's built in, and is stale after", async t => {
  const {page,config,submit,field}=await agentsBox(t,"Agents Edit");
  const reviewer=item(await page(),"agent-reviewer");
  const values=opening(reviewer,"edit");
  assert.deepEqual([values.mode,values["may-edit"],values.allowed,values.asks,values.never],["all","deny","Run commands: git diff *","",""]);
  const saved=await submit(reviewer,"edit",{...values,mode:"subagent","may-edit":"allow",asks:"Edit files: *.lock"});
  assert.equal(saved.status,200,saved.text);
  assert.deepEqual(config().agents.reviewer,{mode:"subagent",description:"Reviews changes\nand says what's wrong",steps:12,
    permissions:[{action:"shell",resource:"git diff *",effect:"allow"},{action:"edit",resource:"*.lock",effect:"ask"}]});
  assert.equal(config().default_agent,undefined);
  assert.equal((await submit(reviewer,"edit",values)).status,409);
  const plan=item(saved.json,"agent-plan");
  const opened=opening(plan,"edit");
  assert.deepEqual([opened.mode,opened.system,opened["may-edit"]],["primary","Plan in short steps.","deny"]);
  assert.deepEqual(field(plan,"edit","may-edit").choices[2],{id:"deny",title:"Don't Allow (Default)"});
  assert.equal((await submit(plan,"edit",opened)).status,200);
  assert.deepEqual(config().agents.plan,{system:"Plan in short steps."});
  const again=item(await page(),"agent-plan");
  assert.equal((await submit(again,"edit",{...opening(again,"edit"),system:"  "})).status,200);
  assert.equal(config().agents.plan,undefined);
});

test("Agents leaves permissions in an order its form can't show, a model written as an object, and one only turned off", async t => {
  const why="Its permissions are in an order only the settings file keeps: change them there.";
  const lists={odd:[{action:"shell",resource:"rm *",effect:"deny"},{action:"shell",resource:"rm -i *",effect:"allow"}],
    late:[{action:"shell",resource:"git *",effect:"allow"},{action:"shell",resource:"*",effect:"deny"}]};
  const {page,config,change,submit,field}=await agentsBox(t,"Agents Kept",{agents:{odd:{mode:"subagent",permissions:lists.odd},late:{mode:"subagent",permissions:lists.late},
    object:{mode:"subagent",model:{providerID:"lab",modelID:"big"}},bare:{disabled:true}}});
  for (const id of ["odd","late"]) {
    const agent=item(await page(),`agent-${id}`);
    const allowed=field(agent,"edit","allowed");
    assert.deepEqual([allowed.value,allowed.can],["",{why}]);
    assert.deepEqual(field(agent,"edit","may-shell").can,{why});
    assert.equal((await submit(agent,"edit",{...opening(agent,"edit"),description:"Careful"})).status,200);
    assert.deepEqual(config().agents[id],{mode:"subagent",description:"Careful",permissions:lists[id]});
  }
  const object=item(await page(),"agent-object");
  assert.equal((await submit(object,"edit",opening(object,"edit"))).status,200);
  assert.deepEqual(config().agents.object.model,{providerID:"lab",modelID:"big"});
  const shown=(await change(item(await page(),"agent-bare"),true)).json;
  assert.deepEqual(config().agents.bare,{disabled:false});
  assert.equal(item(shown,"agent-bare").value,true);
});

test("Agents Reset and Delete take the settings out; agents that aren't an object are left alone and said so", async t => {
  const {box,page,config,submit}=await agentsBox(t,"Agents Reset");
  let shown=(await submit(item(await page(),"agent-plan"),"reset")).json;
  assert.equal(config().agents.plan,undefined);
  assert.deepEqual(item(shown,"agent-plan").actions.map(a=>a.id),["edit"]);
  shown=(await submit(item(shown,"agent-reviewer"),"delete")).json;
  assert.equal(config().agents.reviewer,undefined);
  assert.equal(config().default_agent,undefined);
  assert.deepEqual(config().agents.title,{model:"lab/fast"});
  assert.equal(item(shown,"agent-reviewer"),undefined);
  write(path.join(box.home,"box/opencode.json"),JSON.stringify({agents:["not","an","object"]}));
  const odd=await page();
  assert.equal(row(odd,"agents").add,undefined);
  assert.match(item(odd,"agent-build").can.why,/aren't in a form Kuvo can change/);
  assert.equal(odd.notes[0].tone,"warning");
});

test("Agents offers Claude Code's agents, and one by a built-in one's name comes over beside it", async t => {
  const {box,page,config,submit}=await agentsBox(t,"Agents Claude");
  write(path.join(box.env.HOME,".claude/agents/security.md"),"---\nname: Security Auditor\ndescription: Checks for security problems\n---\nLook for injection.\n");
  write(path.join(box.env.HOME,".claude/agents/plan.md"),"---\nname: plan\ndescription: Claude's planner\n---\nPlan it.\n");
  const shown=await until(async()=>{const p=await page();return item(p,"claude-security-auditor")&&p;},"Claude Code's agents must be offered.");
  const last=shown.groups.at(-1);
  assert.deepEqual([last.id,last.title],["claude-code","From Claude Code"]);
  const security=item(shown,"claude-security-auditor");
  assert.deepEqual([security.title,security.line,security.kind],["Security Auditor","Checks for security problems","actions"]);
  const after=(await submit(security,"import")).json;
  assert.deepEqual(config().agents["security-auditor"],{mode:"subagent",description:"Checks for security problems",system:"Look for injection."});
  assert.deepEqual([item(after,"claude-security-auditor").kind,item(after,"claude-security-auditor").value],["status","Imported"]);
  assert.equal(item(after,"claude-plan-claude").title,"plan");
  assert.equal(item(after,"agent-security-auditor").line,"A subagent · Checks for security problems");
});

// Settings › Skills, with one skill of yours written as the editor writes it.
const skillText=(name,description,body)=>`---\nname: "${name}"\ndescription: "${description}"\n---\n\n${body}\n`;
const skillsBox=async(t,name)=>{
  const {computer}=await bench(t);
  const box=await computer(name);
  write(path.join(box.home,"box/opencode.json"),"{}");
  const record=await box.start().ready();
  const ask=box.connect(record);
  const folder=(await ask("GET","kuvo/skills")).json.folder;
  assert.ok(folder);
  const skill=(id,text)=>write(path.join(folder,id,"SKILL.md"),text);
  skill("release",skillText("release","Write releases","Instructions"));
  const page=async()=>{const r=await ask("GET","kuvo/settings/skills");assert.equal(r.status,200,r.text);return r.json;};
  const config=(value)=>value===undefined?readFileSync(path.join(box.home,"box/opencode.json"),"utf8"):write(path.join(box.home,"box/opencode.json"),JSON.stringify(value));
  const change=(r,value,was=r.revision)=>ask("PATCH","kuvo/settings/skills",{row:r.id,value,was});
  return {box,record,ask,folder,skill,page,config,change};
};

test("Skills lists editable skills, refuses a name taken in the check's words, and keeps unrelated settings", async t => {
  const {ask,page,config,change}=await skillsBox(t,"Skills List");
  const shown=await page();
  const release=item(shown,"skill-release");
  assert.equal(release.value,true);
  assert.deepEqual(release.actions[0].form.fields.map(f=>f.title),["Use it when","Instructions"]);
  assert.equal(shown.groups[0].title,"Skills the Agent Can Use");
  assert.equal(shown.groups.find(g=>g.id==="other-places").note,"A catalog is a web address with an index.json listing its skills.");
  const list=row(shown,"skills");
  const body={was:list.add.form.revision,values:{name:"release",description:"Test",instructions:"Text"}};
  const checked=await ask("POST","kuvo/settings/skills/skills/add/check",body);
  assert.equal(checked.json.ready,false);
  const refused=await ask("POST","kuvo/settings/skills/skills/add",body);
  assert.equal(refused.status,400,refused.text);
  assert.deepEqual(refused.json.fields,checked.json.fields);
  config({unrelated:true});
  assert.equal((await change(release,"no")).status,400);
  const off=await change(release,false);
  assert.equal(off.status,200,off.text);
  assert.equal(item(off.json,"skill-release").value,false);
  assert.equal(JSON.parse(config()).unrelated,true);
  config({permissions:"handwritten"});
  const before=config();
  const held=item(await page(),"skill-release");
  assert.ok(held.can.why);
  assert.equal((await change(held,true)).status,403);
  assert.equal(config(),before);
});

test("Skills edits keep unknown header lines, go stale only for their own file, and the old route refuses a changed one", async t => {
  const {ask,folder,skill,page}=await skillsBox(t,"Skills Edit");
  let at=item(await page(),"skill-release");
  const form=at.actions[0].form.revision;
  skill("release",skillText("release","Outside","Instructions"));
  const stale=await ask("POST","kuvo/settings/skills/skill-release/edit",{was:form,values:{description:"Changed",instructions:"New"}});
  assert.equal(stale.status,409,stale.text);
  skill("release","---\nname: release\ndescription: old\nlicense: MIT\n---\nold\n");
  at=item(await page(),"skill-release");
  skill("other",skillText("other","Other","Other"));
  const next=item(await page(),"skill-release");
  assert.equal(next.actions[0].form.revision,at.actions[0].form.revision);
  const description='Use "foo" with C:\\scripts and apostrophes';
  const saved=await ask("POST","kuvo/settings/skills/skill-release/edit",{was:at.actions[0].form.revision,values:{description,instructions:"New instructions"}});
  assert.equal(saved.status,200,saved.text);
  const text=readFileSync(path.join(folder,"release","SKILL.md"),"utf8");
  assert.match(text,/license: MIT/);
  assert.equal(item(saved.json,"skill-release").line,description);
  const legacy=await ask("PUT","kuvo/skills/release",{was:"old",text:"new"});
  assert.equal(legacy.status,409,legacy.text);
  assert.equal(legacy.json.message,"It changed since you read it.");
});

test("Skills follows its files and settings, checks install links as it submits, and leaves malformed places alone", async t => {
  const {record,ask,skill,page,config}=await skillsBox(t,"Skills Follow");
  const lines=await openLine(record,"","kuvo/settings/skills/events");
  try {
    const frames=lines.frames();
    assert.ok(item((await frames.next()).value,"skill-release"));
    skill("release",skillText("release","Changed externally","New"));
    let next;
    do { next=(await frames.next()).value; } while(item(next,"skill-release").line!=="Changed externally");
    config({permissions:[{action:"skill",resource:"release",effect:"deny"}],skills:["/example"]});
    do { next=(await frames.next()).value; } while(item(next,"skill-release").value!==false || !item(next,"place-0"));
    assert.equal(item(next,"place-0").title,"/example");
  } finally { lines.close(); }
  const install=item(await page(),"install");
  const values={repository:"not-a-repository",path:""};
  const checked=await ask("POST","kuvo/settings/skills/install/install/check",{values});
  const refused=await ask("POST","kuvo/settings/skills/install/install",{was:install.actions[0].form.revision,values});
  assert.equal(refused.status,400,refused.text);
  assert.deepEqual(refused.json.fields,checked.json.fields);
  for (const repository of ["https://example.com/acme/skills","https://github.com/acme/skills/tree/reviewed/release"])
    assert.equal((await ask("POST","kuvo/settings/skills/install/install/check",{values:{repository,path:""}})).json.ready,false,repository);
  assert.equal((await ask("POST","kuvo/settings/skills/install/install/check",{values:{repository:"https://github.com/acme/skills",path:"skills/release"}})).json.ready,true);
  config({skills:"handwritten"});
  const before=config();
  const shown=await page();
  assert.deepEqual(shown.groups,[]);
  assert.equal(shown.notes[0].tone,"warning");
  assert.equal(config(),before);
});

test("Skills lists the engine's own with search, deleted ones with theirs, and skips folders it can't read", async t => {
  const {box,ask,folder,page}=await skillsBox(t,"Skills Found");
  box.engine.skills=Array.from({length:7},(_,i)=>({id:`found-${i}`,name:`Found ${i}`,description:`Use found ${i}`,path:`/builtin/found-${i}/SKILL.md`}));
  const shown=await page();
  const list=row(shown,"skills");
  assert.equal(list.search.placeholder,"Search skills");
  const found=item(shown,"skill-found-0");
  assert.deepEqual([found.title,found.line,found.badge],["Found 0","Use found 0",{words:"Built in",tone:"quiet"}]);
  for (let i=0;i<7;i++) {
    const name=`deleted-${i}`;
    assert.equal((await ask("PUT",`kuvo/skills/${name}`,{text:skillText(name,"For testing","Text"),new:true})).status,200);
    assert.equal((await ask("DELETE",`kuvo/skills/${name}`)).status,200);
  }
  assert.deepEqual(item(await page(),"deleted").search,{placeholder:"Search recently deleted",empty:"No deleted skill matches “{query}”."});
  for (const name of ["empty","a".repeat(129)]) mkdirSync(path.join(folder,name),{recursive:true});
  write(path.join(folder,"a".repeat(129),"SKILL.md"),"Long folder");
  const after=await page();
  assert.equal(item(after,"skill-release").title,"release");
  assert.equal(item(after,"skill-empty"),undefined);
  assert.equal(item(after,`skill-${"a".repeat(129)}`),undefined);
});

// Settings › Storage, over the artifact store the box keeps: one file with an older version.
const sha=data=>createHash("sha256").update(data).digest("hex");
const seedOlder=home=>{
  const doc=path.join(home,"notes.md"),v1="The first version.\n",v2="The second version, longer.\n";
  write(doc,v2);
  const header={id:"art_"+sha(doc).slice(0,24),path:doc,name:"notes.md",kind:"document",type:"text/markdown",
    where:{bots:[],project:home,sessions:["ses_code"],groups:[]},pinned:null,gone:null,updated:"2026-10-26T10:01:00.000Z"};
  const version=(n,text,at)=>({n,at,by:{kind:"agent",session:"ses_code",agent:"engine"},turn:{session:"ses_code",message:"m"+n},via:["write"],
    status:n===1?"added":"modified",edits:1,lines:{added:1,removed:n-1},size:Buffer.byteLength(text),hash:"sha256:"+sha(text),kept:true});
  write(path.join(home,"artifacts/key"),Buffer.alloc(32,0x24));
  for (const text of [v1,v2]) write(path.join(home,"artifacts/copies",sha(text).slice(0,2),sha(text)),text);
  write(path.join(home,"artifacts/log/2026-10.jsonl"),[{a:header,v:version(1,v1,"2026-10-26T10:00:00.000Z")},{a:header,v:version(2,v2,"2026-10-26T10:01:00.000Z")}]
    .map(e=>JSON.stringify({...e,formatVersion:1})).join("\n")+"\n");
};
const storageBox=async(t,name,older=false)=>{
  const {computer}=await bench(t);
  const box=await computer(name);
  if (older) seedOlder(box.home);
  const record=await box.start().ready();
  const plain=box.connect(record);
  const ask=(method,route,body)=>plain(method,route,body,{"x-kuvo-from":"app"});
  const page=async()=>{const r=await ask("GET","kuvo/settings/storage");assert.equal(r.status,200,r.text);return r.json;};
  const change=(r,value,extra={})=>ask("PATCH","kuvo/settings/storage",{row:r.id,was:r.revision,value,...extra});
  // (The disk's free space moves by itself.)
  const kept=async()=>{const {free,...rest}=(await ask("GET","kuvo/artifacts/storage")).json;return rest;};
  return {box,record,ask,page,change,kept};
};

test("Storage describes what agents made in decimal units, floors a budget, and refuses stale, wrong or unavailable changes", async t => {
  const {ask,page,change,kept}=await storageBox(t,"Storage Words");
  const shown=await page();
  const made=shown.groups.find(g=>g.id==="agents-made");
  assert.equal(made.title,"What Agents Made");
  assert.deepEqual(made.rows.map(r=>[r.id,r.title,r.line]),[
    ["copies","Copies of what agents made","Each version of the files agents write or change, so you can see what changed and put one back. Never what looks like a secret, and only the changes for code git keeps."],
    ["older-budget","Older versions keep up to","Past it, the oldest go first. The newest version of each file, memory, skills and what’s kept always stay."],
    ["older-copies","Older versions","None kept."]]);
  const budget=row(shown,"older-budget");
  assert.deepEqual([budget.kind,budget.unit,budget.min],["number",{name:"GB",factor:1000000000,decimals:2,rounding:"nearest"},0]);
  assert.deepEqual(budget.suggestions.map(s=>s.value),[0.25,0.5,1,2,5,10].map(n=>n*1000000000));
  assert.equal(budget.confirmBelow.confirm.title,"Keep less of older versions?");
  assert.equal(budget.confirmBelow.confirm.body,"Older versions take 0 bytes now, so the oldest of their copies go until they fit. The newest version of each file, memory, skills and what’s kept stay.");
  const clear=row(shown,"older-copies").actions[0];
  assert.deepEqual([clear.title,clear.confirm],["Clear…",{title:"Clear older versions?",body:"Their copies go; each stays listed, without one. The newest version of each file, memory, skills and what’s kept stay.",button:"Clear"}]);
  // The default is binary, shown as 1.07 GB, and saved untouched it keeps exactly its bytes.
  assert.equal(budget.value,1073741824);
  assert.equal((await change(budget,budget.value)).status,200);
  assert.equal((await ask("GET","kuvo/artifacts/storage")).json.budget,1073741824);
  const now=row(await page(),"older-budget");
  assert.equal((await change(now,1000.9)).status,200);
  assert.equal((await ask("GET","kuvo/artifacts/storage")).json.budget,1000);
  assert.equal((await change(now,2000)).status,409);
  assert.equal((await change(row(await page(),"older-budget"),"1000")).status,400);
  assert.equal((await ask("POST","kuvo/artifacts/storage",{budget:4000000})).status,200);
  const small=row(await page(),"older-budget");
  assert.deepEqual(small.unit,{name:"MB",factor:1000000,decimals:2,rounding:"nearest"});
  assert.equal((await change(small,small.value)).status,200);
  assert.equal((await ask("GET","kuvo/artifacts/storage")).json.budget,4000000);
  // Clear with nothing to clear says why, and changes nothing.
  const before=await kept();
  const none=row(await page(),"older-copies");
  assert.equal(none.actions[0].can.why,"None kept.");
  const refused=await ask("POST","kuvo/settings/storage/older-copies/clear",{was:none.revision});
  assert.deepEqual([refused.status,refused.json.message],[403,"None kept."]);
  assert.deepEqual(await kept(),before);
});

test("Storage asks before keeping less than older versions take, and follows the page, the old budget route and Clear", async t => {
  const {record,ask,page,change,kept}=await storageBox(t,"Storage Confirm",true);
  const at=row(await page(),"older-budget");
  const before=await kept();
  assert.ok(before.older>0,JSON.stringify(before));
  // A warning read when older versions took less is stale: nothing is written or deleted.
  assert.equal((await ask("POST","kuvo/artifacts/storage",{budget:0,was:before.budget,wasOlder:0,confirmed:true})).status,409);
  assert.deepEqual(await kept(),before);
  for (const confirmed of [undefined,null,false,"true",1]) {
    const answer=await change(at,0,confirmed===undefined?{}:{confirmed});
    assert.equal(answer.status,400,String(confirmed));
    if (confirmed===undefined||confirmed===false) assert.equal(answer.json.message,at.confirmBelow.confirm.body);
    assert.deepEqual(await kept(),before);
  }
  assert.equal((await change(at,at.confirmBelow.value+100)).status,200);
  assert.equal((await change(row(await page(),"older-budget"),at.confirmBelow.value)).status,200);
  assert.equal((await ask("GET","kuvo/artifacts/storage")).json.older,before.older);
  const lines=await openLine(record,"","kuvo/settings/storage/events",{"x-kuvo-from":"app"});
  try {
    const frames=lines.frames();
    const next=async test=>{let frame;do{frame=(await frames.next()).value;}while(!test(frame));return frame;};
    const first=await next(()=>true);
    assert.equal((await change(row(first,"older-budget"),2000000)).status,200);
    await next(p=>row(p,"older-budget").value===2000000);
    assert.equal((await ask("POST","kuvo/artifacts/storage",{budget:3000000})).status,200);
    await next(p=>row(p,"older-budget").value===3000000);
    const bytes=row(await page(),"older-copies").value;
    assert.equal((await ask("POST","kuvo/artifacts/storage/clear",{was:bytes+1})).status,409);
    assert.equal((await ask("GET","kuvo/artifacts/storage")).json.older,bytes);
    assert.equal((await ask("POST","kuvo/artifacts/storage/clear",{})).status,200);
    await next(p=>row(p,"older-copies").value===0);
  } finally { lines.close(); }
});

test("Storage Refresh measures again, saying so at once", async t => {
  const {box,ask,page}=await storageBox(t,"Storage Refresh");
  const used=await until(async()=>{const r=row(await page(),"used");return r?.parts&&r;},"Storage did not measure its files.");
  assert.match(used.title,/^[\d.]+ (bytes|KB|MB|GB) used by Kuvo$/);
  assert.deepEqual(used.actions.map(a=>[a.id,a.title]),[["refresh","Refresh"]]);
  assert.equal(row(await page(),"manage-archived").section,"archived");
  write(path.join(box.home,"browser","more"),Buffer.alloc(400000,1));
  const refreshed=await ask("POST","kuvo/settings/storage/used/refresh",{was:used.revision});
  assert.equal(refreshed.status,200,refreshed.text);
  assert.equal(row(refreshed.json,"used").busy,"Measuring…");
  await until(async()=>row(await page(),"used").parts?.find(p=>p.id==="browser").value>=400000,"Refresh did not measure again.");
});

// Settings › Vault, on a machine's own vault made with a client's signed key request.
const VAULT_SECRET="hunter2-correct-horse";
const vaultHere=process.platform==="darwin"?"this Mac":"this computer";
const CONFIRMS={"x-kuvo-from":"app","x-kuvo-confirms-you":"1"};
const vaultBox=async(t,name,{env,bots={}}={})=>{
  const {computer}=await bench(t);
  const {randomBytes,generateKeyPairSync,sign}=await import("node:crypto");
  const box=await computer(name,boxBinary(),env);
  const signing=generateKeyPairSync("ed25519");
  const mac=randomBytes(32).toString("base64url"),id=randomBytes(12).toString("hex");
  const linkFile=path.join(box.home,"run/link.json");
  write(linkFile,JSON.stringify({...JSON.parse(readFileSync(linkFile,"utf8")),id,macs:[mac],
    macSigns:{[mac]:signing.publicKey.export({type:"spki",format:"der"}).subarray(-32).toString("base64url")}}));
  const at=new Date().toISOString();
  for (const [bid,bname] of Object.entries(bots)) {
    write(path.join(box.home,"bots",bid,"bot.json"),JSON.stringify({kuvo:1,id:bid,name:bname,look:{color:"sky",shape:"gem",face:"wink"},created:at,updated:at}));
    write(path.join(box.home,"bots",bid,"session"),`ses_${bid}`);
  }
  let proc=box.start();
  let record=await proc.ready();
  let plain=box.connect(record);
  const v={box,client:{"x-kuvo-from":"app"}};
  v.record=()=>record;
  v.ask=(method,route,body,headers=v.client)=>plain(method,route,body,headers);
  v.make=async()=>{
    const at=Date.now(),nonce=randomBytes(20).toString("base64url");
    const proof=sign(null,Buffer.from(`kuvo-vault-key\0${id}\0${at}\0${nonce}`),signing.privateKey).toString("base64url");
    const made=await v.ask("POST","kuvo/vault/key",{mac,at,nonce,proof});
    assert.equal(made.status,200,made.text);
    return made.json.key;
  };
  v.add=async item=>{const r=await v.ask("POST","kuvo/vault/items",item);assert.equal(r.status,200,r.text);return r.json.item;};
  v.github=async()=>{await v.make();return v.add({kind:"login",name:"GitHub",sites:["github.com"],username:"octo",values:{password:VAULT_SECRET},signInOnly:true});};
  v.page=async()=>{const r=await v.ask("GET","kuvo/settings/vault");assert.equal(r.status,200,r.text);assert.ok(!r.text.includes(VAULT_SECRET));return r.json;};
  v.act=(r,id,values,was=action(r,id)?.form?.revision??r.revision)=>v.ask("POST",`kuvo/settings/vault/${encodeURIComponent(r.id)}/${id}`,{was,...(values===undefined?{}:{values})});
  v.check=(r,id,values)=>v.ask("POST",`kuvo/settings/vault/${encodeURIComponent(r.id)}/${id}/check`,{was:action(r,id).form.revision,values});
  v.change=(r,value)=>v.ask("PATCH","kuvo/settings/vault",{row:r.id,value,was:r.revision});
  v.items=async()=>(await v.ask("GET","kuvo/vault")).json.items;
  v.activity=async()=>(await v.ask("GET","kuvo/vault/activity")).json;
  v.restart=async()=>{await proc.stop();proc=box.start();record=await proc.ready();plain=box.connect(record);};
  return v;
};
const opened=(r,id)=>Object.fromEntries(action(r,id).form.fields.filter(f=>"value" in f).map(f=>[f.id,f.value]));
const masked=text=>"•".repeat(Array.from(text).length);

test("Vault before there's one says where its first item is added, then lists items by what they are, never a value", async t => {
  const v=await vaultBox(t,"Vault List",{bots:{bot_scout:"Scout"}});
  const listed=(await v.ask("GET","kuvo/settings")).json.sections.find(s=>s.id==="vault");
  assert.deepEqual([listed.title,listed.group,listed.icon],["Vault","Agents & Tools","shield"]);
  let shown=await v.page();
  let items=row(shown,"items");
  assert.deepEqual([items.kind,items.items,items.empty],["list",[],"Nothing yet. Add a sign-in, a key, a file or a note for your agents to use."]);
  assert.equal(items.add.can.why,`There’s no vault on ${vaultHere} yet. This app can’t make one: it doesn’t keep the key that unlocks a vault after its computer restarts.`);
  assert.equal(row(shown,"pause"),undefined);
  assert.equal((await v.act(items,"add",{kind:"note",name:"Contact",text:"Call me maybe"})).status,403);
  const github=await v.github();
  assert.equal((await v.ask("POST","kuvo/vault/grants",{item:github.id,who:{bot:"bot_scout"},how:"own"},CONFIRMS)).status,200);
  await v.add({kind:"key",name:"App Store",sites:[],values:{value:"key-value-1",issuer_id:"issuer-1"}});
  await v.add({kind:"note",name:"Contact",values:{text:"a note to read"},readable:true});
  shown=await v.page();
  items=row(shown,"items");
  assert.deepEqual(items.items.map(i=>[i.title,i.line,i.badge?.words??null]),[
    ["App Store","API Key · issuer_id, value",null],["Contact","Note · Bots may read it",null],["GitHub","Sign-in · octo · github.com","On its own"]]);
  assert.deepEqual(items.items[2].actions.map(a=>a.title),["Edit…","Who Can Use It…","Delete…"]);
  assert.match(shown.groups.find(g=>g.id==="items").note,/encrypted, with a key only the Kuvo that made it keeps/);
  const pause=row(shown,"pause");
  assert.deepEqual([pause.kind,pause.value,pause.can],["switch",false,true]);
  for (const secret of ["key-value-1","issuer-1","a note to read"]) assert.ok(!JSON.stringify(shown).includes(secret));
  // The activity says what this client did plainly.
  assert.equal((await v.ask("GET",`kuvo/vault/items/${github.id}/values`)).status,200);
  assert.ok(row(await v.page(),"activity").items.some(i=>i.title==="Looked at “GitHub”"));
});

test("Vault Add Item checks without the values themselves, refuses in the same words, and adds sign-ins, keys and files", async t => {
  const v=await vaultBox(t,"Vault Add");
  await v.github();
  const items=row(await v.page(),"items");
  assert.deepEqual(action(items,"add").form.fields.map(f=>[f.id,f.kind,f.shown?.is??null]),[
    ["kind","choice",null],["name","text",null],["site","text",["login"]],["username","text",["login"]],["password","secret",["login"]],["signInOnly","switch",["login"]],
    ["hosts","text",["key"]],["value","secret",["key"]],["more","pairs",["key"]],["file","file",["file"]],["text","secret",["note"]],["readable","switch",["note"]]]);
  const open=opened(items,"add");
  const has=(c,id,words,tone)=>c.fields.some(f=>f.id===id&&f.words===words&&f.tone===tone);
  let checked=(await v.check(items,"add",{...open,name:"github",site:"github.com",password:masked("abc")})).json;
  assert.equal(checked.ready,false);
  assert.ok(has(checked,"name","There’s an item called “github” already.","warning"),JSON.stringify(checked));
  assert.ok(has(checked,"password","A value needs at least 4 characters, so it can be hidden from agents.","warning"));
  checked=(await v.check(items,"add",{...open,name:"Linear",site:"not a site!",password:""})).json;
  assert.ok(has(checked,"site","“site!” isn’t a site.","warning"),JSON.stringify(checked));
  assert.ok(has(checked,"password","Its password.","quiet"));
  assert.deepEqual((await v.check(items,"add",{...open,name:"Linear",site:"linear.app",password:masked("linear-pass")})).json,{fields:[],ready:true});
  const refused=await v.act(items,"add",{...open,name:"Linear",site:"linear.app",password:"abc"});
  assert.equal(refused.status,400);
  assert.equal(refused.json.message,"A value needs at least 4 characters, so it can be hidden from agents.");
  assert.ok(!refused.text.includes('"abc"'));
  const added=await v.act(items,"add",{...open,name:"Linear",site:"https://linear.app/login",username:"me",password:VAULT_SECRET});
  assert.equal(added.status,200,added.text);
  assert.ok(!added.text.includes(VAULT_SECRET));
  const linear=(await v.items()).find(i=>i.name==="Linear");
  assert.deepEqual([linear.kind,linear.sites,linear.username,linear.fields,linear.signInOnly],["login",["linear.app"],"me",["password"],true]);
  assert.deepEqual((await v.ask("POST",`kuvo/vault/items/${linear.id}/same`,{values:{password:VAULT_SECRET}})).json,{same:true});
  assert.ok(!JSON.stringify(await v.activity()).includes(VAULT_SECRET));
  const more=await v.act(row(await v.page(),"items"),"add",{...open,kind:"key",name:"Store Key",hosts:"api.example.com",value:"the-key-value",more:[{key:"Issuer ID",value:"issuer-value"}]});
  assert.equal(more.status,200,more.text);
  const key=(await v.items()).find(i=>i.name==="Store Key");
  assert.deepEqual([key.sites,key.fields],[["api.example.com"],["issuer_id","value"]]);
  const big=(await v.check(row(await v.page(),"items"),"add",{...open,kind:"file",name:"Big",file:{name:"big.p8",size:900000}})).json;
  assert.ok(has(big,"file","That file is too big for the vault (at most about 700 KB).","warning"),JSON.stringify(big));
  const sizeOnly=await v.act(row(await v.page(),"items"),"add",{...open,kind:"file",name:"Signing Key",file:{name:"AuthKey.p8",size:1234}});
  assert.equal(sizeOnly.status,400);
  const file=await v.act(row(await v.page(),"items"),"add",{...open,kind:"file",name:"Signing Key",file:{name:"AuthKey.p8",data:Buffer.from("-----BEGIN KEY-----").toString("base64")}});
  assert.equal(file.status,200,file.text);
  const kept=(await v.items()).find(i=>i.name==="Signing Key");
  assert.deepEqual([kept.fileName,kept.fields],["AuthKey.p8",["file"]]);
  assert.equal(item(file.json,`item-${kept.id}`).line,"File · AuthKey.p8");
});

test("Vault Edit opens empty for each value, saves only what changed, goes stale, and takes a key's other field away", async t => {
  const v=await vaultBox(t,"Vault Edit");
  const github=await v.github();
  let r=item(await v.page(),`item-${github.id}`);
  const password=action(r,"edit").form.fields.find(f=>f.id==="password");
  assert.deepEqual(password,{id:"password",title:"Password",kind:"secret",placeholder:"Unchanged",shows:{route:`kuvo/vault/items/${github.id}/values`,field:"password"}});
  assert.deepEqual(action(r,"edit").form.fields.map(f=>f.id),["name","site","username","password","signInOnly"]);
  const shown=await v.ask("GET",password.shows.route);
  assert.equal(shown.json.values.password,VAULT_SECRET);
  const count=JSON.stringify(await v.activity()).length;
  const changed=(await v.items())[0].changed;
  assert.equal((await v.act(r,"edit",{...opened(r,"edit"),password:""})).status,200);
  assert.equal((await v.act(r,"edit",{...opened(r,"edit"),password:VAULT_SECRET})).status,200);
  assert.equal(JSON.stringify(await v.activity()).length,count);
  assert.equal((await v.items())[0].changed,changed);
  const saved=await v.act(r,"edit",{...opened(r,"edit"),username:"octocat",password:"new-password-1"});
  assert.equal(saved.status,200,saved.text);
  const now=(await v.items())[0];
  assert.deepEqual([now.username,now.sites,now.signInOnly],["octocat",["github.com"],true]);
  assert.deepEqual((await v.ask("POST",`kuvo/vault/items/${github.id}/same`,{values:{password:"new-password-1"}})).json,{same:true});
  const stale=await v.act(r,"edit",{...opened(r,"edit"),name:"GitHub 2"});
  assert.equal(stale.status,409);
  assert.equal(item(stale.json.page,`item-${github.id}`).title,"GitHub");
  r=item(await v.page(),`item-${github.id}`);
  await v.add({kind:"note",name:"Other",values:{text:"other note"}});
  assert.equal((await v.act(r,"edit",{...opened(r,"edit"),name:"GitHub Work"})).status,200);
  assert.equal((await v.items()).find(i=>i.id===github.id).name,"GitHub Work");
  const key=await v.add({kind:"key",name:"Store Key",sites:[],values:{value:"the-key-value",isuer_id:"typo-value"}});
  r=item(await v.page(),`item-${key.id}`);
  const remove=action(r,"edit").form.fields.find(f=>f.id==="remove");
  assert.deepEqual([remove.kind,remove.value,remove.choices],["choice","none",[{id:"none",title:"None"},{id:"isuer_id",title:"Isuer Id"}]]);
  assert.equal((await v.act(r,"edit",{...opened(r,"edit"),remove:"isuer_id"})).status,200);
  assert.deepEqual((await v.items()).find(i=>i.id===key.id).fields,["value"]);
  // Delete… takes the item away.
  r=item(await v.page(),`item-${github.id}`);
  const del=action(r,"delete");
  assert.deepEqual([del.style,del.confirm.title,del.confirm.button],["destructive","Delete “GitHub Work” from the vault?","Delete"]);
  const done=await v.act(r,"delete");
  assert.equal(done.status,200,done.text);
  assert.equal(item(done.json,`item-${github.id}`),undefined);
  assert.equal((await v.items()).some(i=>i.id===github.id),false);
});

test("Vault Who Can Use It takes access away from any client and gives more only from one that makes sure it's you", async t => {
  const v=await vaultBox(t,"Vault Who",{bots:{bot_scout:"Scout",bot_ranger:"Ranger"}});
  const github=await v.github();
  assert.equal((await v.ask("POST","kuvo/vault/grants",{item:github.id,who:{bot:"bot_scout"},how:"ask"},CONFIRMS)).status,200);
  const why="This needs Touch ID or your Mac’s password. Update Kuvo to do it here.";
  let r=item(await v.page(),`item-${github.id}`);
  let who=action(r,"who").form;
  assert.deepEqual(who.fields.map(f=>[f.id,f.value,f.choices.map(c=>c.id),f.can?.why??null]),[
    ["bot-bot_scout","ask",["ask","no"],null],["everyone","no",["no"],why],["add-bot","none",["none","bot_ranger"],why]]);
  assert.equal(who.note,"Only on github.com. Agents never see the password: Kuvo fills it in.");
  assert.equal((await v.act(r,"who",{...opened(r,"who"),"bot-bot_scout":"own"})).json.message,why);
  assert.equal((await v.act(r,"who",{...opened(r,"who"),"add-bot":"bot_ranger"})).status,403);
  const grants=async()=>(await v.ask("GET","kuvo/vault/grants")).json.grants;
  assert.deepEqual((await grants()).map(g=>[g.who,g.how]),[[{bot:"bot_scout"},"ask"]]);
  assert.equal((await v.act(r,"who",{...opened(r,"who"),"bot-bot_scout":"no"})).status,200);
  assert.deepEqual(await grants(),[]);
  v.client=CONFIRMS;
  r=item(await v.page(),`item-${github.id}`);
  who=action(r,"who").form;
  assert.deepEqual(who.fields.map(f=>[f.id,f.title,f.can??null,f.choices.map(c=>[c.id,c.verified??false])]),[
    ["everyone","Every agent",null,[["own",true],["ask",true],["no",false]]],
    ["add-bot","Let a Bot Use It",null,[["none",false],["bot_ranger",true],["bot_scout",true]]]]);
  assert.deepEqual(who.fields[1].choices.map(c=>c.title),["No One Else","Ranger","Scout"]);
  assert.equal((await v.act(r,"who",{...opened(r,"who"),"add-bot":"bot_scout"})).status,200);
  r=item(await v.page(),`item-${github.id}`);
  assert.deepEqual(r.badge,{words:"Asks",tone:"quiet"});
  assert.equal((await v.act(r,"who",{...opened(r,"who"),"bot-bot_scout":"own"})).status,200);
  assert.deepEqual((await grants()).map(g=>[g.who,g.how]),[[{bot:"bot_scout"},"own"]]);
  r=item(await v.page(),`item-${github.id}`);
  assert.equal((await v.act(r,"who",{everyone:"always"})).status,400);
});

test("Vault Pause stops every agent's use and lets a paused bot go on; the page follows without a value", async t => {
  const v=await vaultBox(t,"Vault Pause",{bots:{bot_scout:"Scout"}});
  const github=await v.github();
  assert.equal((await v.ask("POST","kuvo/vault/pause",{paused:true,bot:"bot_scout"})).status,200);
  let shown=await v.page();
  const paused=item(shown,"paused-bot_scout");
  assert.deepEqual([paused.title,paused.actions.map(a=>[a.id,a.title])],["Scout is paused",[["go-on","Let Go On"]]]);
  shown=(await v.change(row(shown,"pause"),true)).json;
  assert.equal(row(shown,"pause").value,true);
  assert.equal((await v.ask("GET","kuvo/vault")).json.state.paused,true);
  shown=(await v.act(item(shown,"paused-bot_scout"),"go-on")).json;
  assert.equal(item(shown,"paused-bot_scout"),undefined);
  assert.deepEqual((await v.ask("GET","kuvo/vault")).json.state.pausedBots??{},{});
  const titles=row(shown,"activity").items.map(i=>i.title);
  assert.deepEqual(titles.slice(0,2),["Scout’s use of the vault goes on again","The vault was paused"]);
  const lines=await openLine(v.record(),"","kuvo/settings/vault/events",{"x-kuvo-from":"app"});
  try {
    const frames=lines.frames();
    const first=(await frames.next()).value;
    const r=item(first,`item-${github.id}`);
    assert.equal(r.title,"GitHub");
    assert.equal((await v.act(r,"edit",{...opened(r,"edit"),name:"GitHub Work",password:"another-secret-9"})).status,200);
    let next;
    do { next=(await frames.next()).value; } while(item(next,`item-${github.id}`).title!=="GitHub Work");
    assert.ok(!JSON.stringify(next).includes("another-secret-9"));
  } finally { lines.close(); }
});

test("Vault locked after a restart says so and shows nothing; a request that isn't a client's reads nothing; no form carries a value", async t => {
  const v=await vaultBox(t,"Vault Locked",{bots:{bot_scout:"Scout"}});
  await v.github();
  v.client=CONFIRMS;
  const listed=(await v.ask("GET","kuvo/settings")).json;
  for (const section of listed.sections) {
    const shown=(await v.ask("GET",`kuvo/settings/${section.id}`)).json;
    const rows=(shown.groups??[]).flatMap(g=>g.rows.flatMap(r=>[r,...(r.items??[])]));
    const forms=rows.flatMap(r=>[...(r.actions??[]),...(r.add?[r.add]:[]),...(r.flow?.actions??[])]).flatMap(a=>a.form?[a.form]:[]);
    for (const field of forms.flatMap(f=>f.fields)) {
      if (field.kind==="secret") assert.equal("value" in field,false,section.id);
      if (field.kind==="pairs"&&field.secret) assert.ok((field.value??[]).every(p=>p.value===""));
      if (field.kind==="file") assert.ok(Object.keys(field.value??{}).every(k=>k==="name"));
      if (field.shows) assert.match(field.shows.route,/^kuvo\/vault\/items\/v_[a-z0-9]{10}\/values$/);
    }
    assert.ok(!JSON.stringify(shown).includes(VAULT_SECRET),section.id);
  }
  const outside=(await v.ask("GET","kuvo/settings/vault",undefined,{})).json;
  assert.deepEqual(outside.groups,[]);
  // (Named as this computer goes by: TypeScript says its host name, Rust its name in the setup.)
  assert.match(outside.notes[0].words,/^Only Kuvo on your devices can open .+’s vault\.$/);
  await v.restart();
  v.client={"x-kuvo-from":"app"};
  const locked=await v.page();
  assert.deepEqual(locked.groups,[]);
  assert.deepEqual(locked.notes,[{words:`${vaultHere[0].toUpperCase()+vaultHere.slice(1)}’s vault is locked since it started again, so its agents can’t use it. Unlocking it takes the key it was made with, which this app doesn’t keep.`,tone:"warning"}]);
});

test("Vault answers what agents wait on you for from the page: once from any client, from now on only from one that makes sure it's you", {skip:process.platform==="win32"}, async t => {
  const v=await vaultBox(t,"Vault Asks",{env:()=>({KUVO_VAULT_AGENTS:"1",KUVO_TEST_LOGIN_ENV:"off"})});
  const github=await v.github();
  const project=path.join(v.box.home,"project");
  mkdirSync(project,{recursive:true});
  const session=sessionOf({id:"ses_asker",title:"Release",directory:project});
  v.box.engine.sessions.push(session);
  const args={need:"an npm token",why:"To publish"};
  v.box.engine.messages.set(session.id,[{id:"msg_vault",content:[{type:"tool",name:"vault_ask",callID:"call_ask",state:{status:"running",input:args}}]}]);
  const waiting=v.ask("POST","kuvo/vault/tools",{session:session.id,name:"vault_ask",arguments:args});
  const why="This needs Touch ID or your Mac’s password. Update Kuvo to do it here.";
  const shown=await until(async()=>{const p=await v.page();return p.groups[0]?.id!=="items"&&p.groups[0]?.rows?.length&&p;},"The ask must be on the page.");
  const group=shown.groups[0];
  assert.equal(group.title,"Waiting for You");
  const asked=group.rows[0];
  assert.match(asked.title,/needs an npm token$/);
  assert.equal(asked.line,"To publish");
  assert.deepEqual(asked.actions.map(a=>a.title),["Don’t Allow","Give…"]);
  const how=action(asked,"give").form.fields.find(f=>f.id==="how");
  assert.deepEqual(how.can,{why});
  assert.equal((await v.act(asked,"give",{item:github.id,how:"always"})).status,403);
  v.client=CONFIRMS;
  const offered=item(await v.page(),asked.id);
  assert.deepEqual(action(offered,"give").form.fields.map(f=>[f.id,f.value,f.choices.map(c=>c.verified??false)]),[["item","",[false]],["how","once",[false,true]]]);
  assert.equal((await v.act(offered,"give",{item:github.id,how:"once"})).status,200);
  const supplied=await waiting;
  assert.equal(supplied.json.isError,false,supplied.text);
  assert.equal(item(await v.page(),asked.id),undefined);
  // One answered elsewhere meanwhile is gone from the page: refused, nothing answered.
  assert.equal((await v.act(offered,"give",{item:github.id,how:"once"})).status,404);
});

// Settings › Permissions, on a computer whose engine settings hold one rule of yours.
const permissionsBox=async(t,name)=>{
  const {computer}=await bench(t);
  const box=await computer(name);
  const file=path.join(box.home,"box/opencode.json");
  write(file,JSON.stringify({permissions:[{action:"shell",resource:"git push *",effect:"ask"}]}));
  const record=await box.start().ready();
  const ask=box.connect(record);
  const page=async()=>{const r=await ask("GET","kuvo/settings/permissions");assert.equal(r.status,200,r.text);return r.json;};
  const change=(r,value,was=r.revision)=>ask("PATCH","kuvo/settings/permissions",{row:r.id,value,was});
  const rules=value=>value===undefined?JSON.parse(readFileSync(file,"utf8")).permissions:write(file,JSON.stringify({permissions:value}));
  const approval=path.join(box.home,"box/approval.json");
  const kept=()=>existsSync(approval)?readFileSync(approval,"utf8"):null;
  return {box,record,ask,page,change,rules,file,approval,kept};
};

test("Permissions Full Access can't be turned on here and says why, but turns off as kuvo/approval does; other computers keep the old route's file", async t => {
  const {ask,page,change,kept,approval}=await permissionsBox(t,"Full Access");
  const off=row(await page(),"full-access");
  assert.deepEqual([off.kind,off.value],["switch",false]);
  assert.match(off.can.why,/pick Full Access in its mode menu/);
  const refused=await change(off,true);
  assert.equal(refused.status,403);
  assert.equal(refused.json.message,off.can.why);
  assert.equal((await ask("GET","kuvo/approval")).json.fullAccess,false);
  await ask("PATCH","kuvo/approval",{fullAccess:true});
  await ask("PATCH","kuvo/approval",{fullAccess:false});
  const byRoute=kept();
  await ask("PATCH","kuvo/approval",{fullAccess:true});
  const on=row(await page(),"full-access");
  assert.deepEqual([on.value,on.can,on.confirm.button],[true,true,"Turn Off"]);
  assert.equal((await change(on,false)).status,200);
  assert.equal(kept(),byRoute);
  rmSync(approval,{force:true});
  await ask("PATCH","kuvo/approval",{otherComputers:false});
  const otherByRoute=kept();
  rmSync(approval,{force:true});
  const other=row(await page(),"other-computers");
  assert.deepEqual([other.kind,other.value],["switch",true]);
  assert.match(other.line,/^Their bots and agents can start sessions on .+\.$/);
  // The same value through the old route leaves its revision.
  await ask("PATCH","kuvo/approval",{otherComputers:true});
  const same=row(await page(),"other-computers");
  assert.equal(same.revision,other.revision);
  rmSync(approval,{force:true});
  const done=await change(other,false);
  assert.equal(done.status,200,done.text);
  assert.equal(row(done.json,"other-computers").value,false);
  assert.equal(kept(),otherByRoute);
});

test("Permissions writes each choice as a rule for everything, keeps the others, refuses a stale one, and leaves rules it can't list", async t => {
  const {page,change,rules}=await permissionsBox(t,"Permission Rules");
  let shown=await page();
  const shell=row(shown,"when-shell");
  assert.deepEqual([shell.kind,shell.value],["choice","allow"]);
  assert.deepEqual(shell.choices.map(c=>c.title),["Allow (Default)","Ask First","Don't Allow"]);
  assert.deepEqual(row(shown,"when-external_directory").choices.map(c=>c.title),["Allow","Ask First (Default)","Don't Allow"]);
  shown=(await change(shell,"deny")).json;
  assert.deepEqual(rules(),[{action:"shell",resource:"*",effect:"deny"},{action:"shell",resource:"git push *",effect:"ask"}]);
  assert.equal(row(shown,"when-shell").value,"deny");
  const read=await page();
  rules([{action:"shell",resource:"*",effect:"deny"},{action:"read",resource:"*.pem",effect:"deny"}]);
  shown=(await change(row(read,"when-shell"),"allow")).json;
  assert.deepEqual(rules(),[{action:"read",resource:"*.pem",effect:"deny"}]);
  assert.equal(row(shown,"when-shell").value,"allow");
  const before=await page();
  rules([{action:"shell",resource:"*",effect:"ask"}]);
  const stale=await change(row(before,"when-shell"),"deny");
  assert.equal(stale.status,409);
  assert.equal(row(stale.json.page,"when-shell").value,"ask");
  assert.equal((await change(row(before,"when-edit"),"ask")).status,200);
  assert.deepEqual(rules(),[{action:"edit",resource:"*",effect:"ask"},{action:"shell",resource:"*",effect:"ask"}]);
  rules({shell:"ask"});
  const unlistedPage=await page();
  const unlisted=row(unlistedPage,"when-shell");
  assert.deepEqual([unlisted.kind,unlisted.value],["status","Set in Its Settings File"]);
  assert.match(unlistedPage.groups.find(g=>g.id==="when").note,/aren't a list of rules/);
  assert.equal((await change(unlisted,"deny")).status,403);
  assert.deepEqual(rules(),{shell:"ask"});
});

test("Permissions picking what applies with no rule writes none, so Plan mode's own rule is never widened; the page follows the file", async t => {
  const {record,page,change,rules,file}=await permissionsBox(t,"Plan Rule");
  const untouched=readFileSync(file,"utf8");
  const edit=row(await page(),"when-edit");
  assert.equal(edit.value,"allow");
  assert.equal((await change(edit,"allow")).status,200);
  assert.equal(readFileSync(file,"utf8"),untouched);
  rules([{action:"edit",resource:"*",effect:"allow"},{action:"shell",resource:"git push *",effect:"ask"}]);
  const own=row(await page(),"when-edit");
  assert.equal(own.value,"allow");
  assert.deepEqual(own.choices.map(c=>[c.id,c.title]),[["allow","Allow (Your Rule)"],["ask","Ask First"],["deny","Don't Allow"],["default","Allow (Default)"]]);
  assert.notEqual(own.revision,edit.revision);
  const kept=readFileSync(file,"utf8");
  assert.equal((await change(own,"allow")).status,200);
  assert.equal(readFileSync(file,"utf8"),kept);
  assert.equal((await change(row(await page(),"when-edit"),"default")).status,200);
  assert.deepEqual(rules(),[{action:"shell",resource:"git push *",effect:"ask"}]);
  assert.equal((await change(row(await page(),"when-edit"),"deny")).status,200);
  assert.deepEqual(rules()[0],{action:"edit",resource:"*",effect:"deny"});
  assert.equal((await change(row(await page(),"when-edit"),"allow")).status,200);
  assert.deepEqual(rules(),[{action:"shell",resource:"git push *",effect:"ask"}]);
  const events=await openLine(record,"","kuvo/settings/permissions/events");
  try {
    const frames=events.frames();
    assert.equal(row((await frames.next()).value,"when-webfetch").value,"allow");
    rules([{action:"webfetch",resource:"*",effect:"deny"}]);
    let next;
    do { next=(await frames.next()).value; } while(row(next,"when-webfetch").value!=="deny");
  } finally { events.close(); }
});

const gitBox=async(t,name,settings="{}")=>{
  const {computer}=await bench(t);
  const box=await computer(name);
  const file=path.join(box.home,"box/opencode.json");
  write(file,settings);
  const record=await box.start().ready();
  const ask=box.connect(record);
  const page=async()=>{const r=await ask("GET","kuvo/settings/git");assert.equal(r.status,200,r.text);return r.json;};
  const change=(r,value,was=r.revision)=>ask("PATCH","kuvo/settings/git",{row:r.id,value,was});
  return {box,record,ask,page,change,file};
};

test("Git: the prefix in a card of its own, instructions under their own headings, cleaned as they're used, and stale writes refused", async t => {
  const {ask,page,change}=await gitBox(t,"Git Rows");
  let shown=await page();
  assert.deepEqual(shown.groups.flatMap(g=>g.rows.map(r=>[r.id,r.kind,r.title,r.value])),[
    ["branch-prefix","text","Branch prefix","kuvo/"],["commit-instructions","text","Commit instructions",""],["pull-request-instructions","text","Pull request instructions",""]]);
  assert.deepEqual(shown.groups.map(g=>[g.id,g.title,g.note,g.rows.length]),[
    ["branches",undefined,undefined,1],
    ["commit-messages","Commit Instructions","Added when Suggest a Message in Changes asks the model for one.",1],
    ["pull-requests","Pull Request Instructions","Added when Create Pull Request writes the title and description.",1]]);
  assert.deepEqual(shown.groups.flatMap(g=>g.rows.map(r=>[r.title,r.line,r.placeholder,r.lines])),[
    ["Branch prefix","How a session’s own branch in a worktree starts.","None",undefined],
    ["Commit instructions",undefined,"For example: Start with the area changed, like “ui: …”, and never mention tickets.",{min:5,max:12}],
    ["Pull request instructions",undefined,"For example: Add a Testing section, and link the issue if the branch names one.",{min:5,max:12}]]);
  const prefix=row(shown,"branch-prefix");
  const answer=await change(prefix,false);
  assert.deepEqual([answer.status,answer.json.message],[400,"Send text."]);
  assert.equal((await change(prefix,"feature/")).status,200);
  assert.equal((await ask("GET","kuvo/config")).json.branch_prefix,"feature/");
  assert.equal((await change(prefix,"other/")).status,409);
  assert.equal((await change(row(shown,"commit-instructions"),"Use imperative mood")).status,200);
  shown=(await change(row(await page(),"branch-prefix"),"")).json;
  assert.equal((await ask("GET","kuvo/config")).json.branch_prefix,"");
  shown=(await change(row(shown,"branch-prefix")," /bad prefix../ ")).json;
  assert.equal(row(shown,"branch-prefix").value,"badprefix./");
  shown=(await change(row(shown,"commit-instructions"),"  Keep it short\n ")).json;
  assert.equal(row(shown,"commit-instructions").value,"Keep it short");
  const config=(await ask("GET","kuvo/config")).json;
  assert.deepEqual([config.branch_prefix,config.commit_instructions],["badprefix./","Keep it short"]);
});

test("Git follows the page, kuvo/config and the one-time move, and leaves settings it can't read or change", async t => {
  const {record,ask,page,change,file}=await gitBox(t,"Git Follow");
  const events=await openLine(record,"","kuvo/settings/git/events");
  try {
    const frames=events.frames();
    const next=async test=>{let f;do{f=(await frames.next()).value;}while(!test(f));return f;};
    const first=await next(()=>true);
    assert.equal((await change(row(first,"branch-prefix"),"session/")).status,200);
    await next(p=>row(p,"branch-prefix").value==="session/");
    assert.equal((await ask("PATCH","kuvo/config",{changes:[{path:["pull_request_instructions"],value:"Include testing"}]})).status,200);
    await next(p=>row(p,"pull-request-instructions").value==="Include testing");
  } finally { events.close(); }
  write(file,JSON.stringify({branch_prefix:false}));
  const before=readFileSync(file,"utf8");
  const prefix=row(await page(),"branch-prefix");
  assert.match(prefix.can.why,/other than text/);
  assert.equal((await change(prefix,"feature/")).status,403);
  assert.equal(readFileSync(file,"utf8"),before);
  write(file,"{broken");
  const shown=await page();
  assert.deepEqual(shown.groups,[]);
  assert.equal(shown.notes[0].tone,"warning");
  assert.match(shown.notes[0].words,/settings/);
});

test("Git events follow the one-time move of a Mac's settings", async t => {
  const {record,ask}=await gitBox(t,"Git Move");
  const events=await openLine(record,"","kuvo/settings/git/events");
  try {
    const frames=events.frames();
    await frames.next();
    const moved=await ask("POST","kuvo/git/settings/move",{branchPrefix:"moved/",commitInstructions:"Keep it short"});
    assert.equal(moved.status,200,moved.text);
    assert.equal(moved.json.moved,true);
    let next;
    do { next=(await frames.next()).value; } while(row(next,"branch-prefix").value!=="moved/");
    assert.equal(row(next,"commit-instructions").value,"Keep it short");
  } finally { events.close(); }
});

test("This Computer and the engine's pages read quickly while the engine doesn't answer", async t => {
  const {computer}=await bench(t);
  const box=await computer("Engine Quiet");
  const ask=box.connect(await box.start().ready());
  await box.engine.close();
  for (const section of ["computer","agents","mcp","skills","worktrees"]) {
    const started=Date.now();
    const answer=await ask("GET","kuvo/settings/"+section);
    assert.equal(answer.status,200,answer.text);
    assert.ok(Date.now()-started<3000,`${section} took ${Date.now()-started} ms`);
  }
});

test("Custom Auto's words, list moves only where they go, its last list kept, edits to a model's effort, and the page followed", async t => {
  const {computer} = await bench(t);
  const box=await computer("Model Words");
  box.engine.providers=[{id:"opencode"}];
  box.engine.models=[{providerID:"opencode",id:"fast",name:"Fast",variants:[{id:"high"}]},{providerID:"opencode",id:"other",name:"Other",variants:[{id:"low"}]}];
  box.engine.integrations=[{id:"opencode",name:"OpenCode Zen",connections:[{}]}];
  const {startClaudeBridge}=await import("./claude.mjs");
  const bridge=await startClaudeBridge(boxBinary(),box.home,box.env,{engine:box.engine});
  t.after(async()=>{await bridge?.stop()});
  const record=await box.start().ready();
  const ask=box.connect(record);
  assert.equal((await ask("GET","kuvo/agents/api/kuvo/auto")).status,200);
  const page=async()=>{const r=await ask("GET","kuvo/settings/custom-auto");assert.equal(r.status,200,r.text);return r.json;};
  let lists=row(await page(),"lists");
  assert.equal((await perform(ask,"custom-auto",lists,"add",{name:"Features"})).status,200);
  // (The list a computer starts with goes; the last one left stays.)
  for (const each of row(await page(),"lists").items.filter(i=>i.title!=="Features"))
    assert.equal((await perform(ask,"custom-auto",item(await page(),each.id),"remove")).status,200);
  const only=row(await page(),"lists").items[0];
  assert.equal(only.title,"Features");
  const refused=await perform(ask,"custom-auto",only,"remove");
  assert.equal(refused.status,403,refused.text);
  assert.equal(row(await page(),"lists").items.length,1);
  lists=row(await page(),"lists");
  const events=await openLine(record,"","kuvo/settings/custom-auto/events");
  try {
    const frames=events.frames();
    await frames.next();
    assert.equal((await perform(ask,"custom-auto",lists,"add",{name:"Bugs"})).status,200);
    let next;
    do { next=(await frames.next()).value; } while(!row(next,"lists").items.some(i=>i.title==="Bugs"));
  } finally { events.close(); }
  const shown=await page();
  assert.equal(shown.title,"Custom Auto");
  assert.equal(shown.groups[0].note,"A model at its limit is skipped until the limit resets; an account at its limit skips all its models.");
  const [first,second]=row(shown,"lists").items;
  assert.deepEqual(row(shown,"lists").items.map(i=>i.title),["Features","Bugs"]);
  const firstId=first.id.slice("list-".length),secondId=second.id.slice("list-".length);
  assert.equal(row(shown,"models-"+secondId).empty,"Add the models to go through, in the order you want them. Pick this list in a session's model picker to use it.");
  const name=action(row(shown,"lists"),"add").form.fields[0];
  assert.deepEqual([name.title,name.line],["Name","Its name, as the model picker shows it"]);
  assert.deepEqual(first.actions.map(a=>a.title),["Edit…","Move Down","Delete List"]);
  assert.equal(second.actions.some(a=>a.id==="down"),false);
  assert.equal(first.actions.some(a=>a.id==="up"),false);
  const one=await perform(ask,"custom-auto",row(shown,"models-"+firstId),"add-model",{model:JSON.stringify(["opencode","fast"]),variant:"high"});
  assert.equal(one.status,200,one.text);
  const entry=item(one.json,`entry-${firstId}-0`);
  assert.equal(entry.actions.some(a=>["up","down"].includes(a.id)),false);
  assert.ok(!JSON.stringify(one.json).includes("Already first.")&&!JSON.stringify(one.json).includes("Already last."));
  const form=action(entry,"edit-model").form;
  const variant=form.fields.find(f=>f.id==="variant");
  assert.deepEqual([form.fields.length,variant.kind,variant.value],[2,"text","high"]);
  const values={model:JSON.stringify(["opencode","other"]),variant:"low"};
  const checked=await ask("POST",`kuvo/settings/custom-auto/entry-${firstId}-0/edit-model/check`,{values});
  assert.equal(checked.json.ready,true,checked.text);
  assert.equal(checked.json.fields.find(f=>f.id==="variant").words,"Supported efforts: low. Leave empty for the default.");
  assert.equal((await perform(ask,"custom-auto",entry,"edit-model",values)).status,200);
  const kept=(await ask("GET","kuvo/agents/api/kuvo/auto")).json.data.lists.find(l=>l.id===firstId);
  assert.deepEqual(kept.entries.map(e=>[e.providerID,e.modelID,e.variant]),[["opencode","other","low"]]);
});

test("a Settings change body is checked before anything is read or written", async t => {
  const {computer}=await bench(t);
  const box=await computer("Change Shape");
  const ask=box.connect(await box.start().ready());
  const at=row((await ask("GET","kuvo/settings/conversations")).json,"compaction");
  const change={row:at.id,value:"600000",was:at.revision};
  for (const body of [{...change,confirmed:"true"},{...change,extra:true},{row:at.id,was:at.revision},{...change,was:1}]) {
    const answer=await ask("PATCH","kuvo/settings/conversations",body);
    assert.equal(answer.status,400,JSON.stringify(body)+" "+answer.text);
  }
  assert.equal(existsSync(path.join(box.home,"box/conversation-compaction.json")),false);
  for (const body of [{...change,confirmed:false},change]) assert.equal((await ask("PATCH","kuvo/settings/conversations",{...body,was:row((await ask("GET","kuvo/settings/conversations")).json,"compaction").revision})).status,200);
});
