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
