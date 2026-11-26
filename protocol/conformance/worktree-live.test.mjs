// Opt-in, one real model turn in a checkout made by the resident. The sign-in is a
// non-renewing test copy, and all engine state and Git data live in the scratch home.
import assert from "node:assert/strict";
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, isolatedEnv, launch, localRelay, privateHome, request, scratch, startBox, until, write } from "./adapter.mjs";
import { git } from "./engine.mjs";

const pinned = process.env.KUVO_CONFORMANCE_OPENCODE;
const credential = process.env.KUVO_CONFORMANCE_OPENCODE_GO;
const model = { providerID: "opencode-go", id: "deepseek-v4.1-flash" };

test("a real OpenCode Go session reads the checkout it works in and archives it", {
  skip: !(pinned && credential) && "Set the isolated live test inputs.",
  timeout: 180000,
}, async () => {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  const project = path.join(root, "project");
  privateHome(home);
  const env = isolatedEnv(root);
  write(path.join(home,"folder-version.json"), '{"version":23}\n');
  write(path.join(home,"box","opencode.json"), "{}\n");
  write(path.join(home,"box","projects.json"), JSON.stringify({projects:[{directory:project,name:"Live fixture"}]}));
  write(path.join(project,"README.md"), "Fixture project\n");
  git(env,project,"init","-q","-b","main");
  git(env,project,"add","-A");
  git(env,project,"commit","-qm","First");
  const reserve = net.createServer();
  await new Promise(resolve => reserve.listen(0,"127.0.0.1",resolve));
  const port = reserve.address().port;
  await new Promise(resolve => reserve.close(resolve));
  const password = "scratch-worktree-engine";
  const relay = await localRelay();
  let worker, box;
  try {
    const given = JSON.parse(fs.readFileSync(credential,"utf8"));
    if (given.integrationID !== model.providerID || given.value?.type !== "key" || given.value.refresh) throw new Error("Use only a non-renewing scratch Go credential.");
    worker = launch(fs.realpathSync(pinned), ["serve", "--port", String(port), "--hostname", "127.0.0.1"], {
      ...env, OPENCODE_PASSWORD:password, OPENCODE_SERVER_PASSWORD:password,
      XDG_DATA_HOME:path.join(root,"opencode-data"), XDG_CONFIG_HOME:path.join(root,"opencode-config"),
      XDG_CACHE_HOME:path.join(root,"opencode-cache"), XDG_STATE_HOME:path.join(root,"opencode-state"),
    }, project);
    const engine = {port,password};
    await until(async () => (await request(engine,"GET","api/provider").catch(()=>null))?.status===200,"the isolated engine to answer",30000);
    const seeded=await request(engine,"POST","api/credential",{integrationID:model.providerID,label:"Scratch worktree proof",value:given.value,activate:true});
    assert.ok(seeded.status===200 || seeded.status===201, "The scratch credential was registered");
    await until(async()=> (await request(engine,"GET","api/model")).json?.data?.some(row=>row.providerID===model.providerID&&row.id===model.id&&row.enabled!==false),"the Go model to be ready",20000);
    write(path.join(home,"run","engine.json"),JSON.stringify({port,password,pid:worker.child.pid,version:"2.2.6"}));
    box = startBox({binary:boxBinary(),home,env:{...env,KUVO_SANDBOX:"off"},relay});
    const record=await box.ready();
    const call=(method,route,body)=>request(record,method,route,body,{"x-kuvo-from":"app"},60000);
    const started=await call("POST","kuvo/sessions",{project:"Live fixture",worktree:true,title:"Read checkout note",model,approval:"askFirst"});
    assert.equal(started.status,200,started.text);
    const {session,folder,worktree}=started.json;
    assert.notEqual(folder,project);
    const token="Checkout note 4731";
    write(path.join(folder,"checkout-note.txt"),token+"\n");
    const sent=await call("POST","kuvo/sessions/"+session+"/send",{id:"msg_worktreelive",words:"Read the file checkout-note.txt in your working folder and reply with only its contents.",delivery:"steer"});
    assert.equal(sent.status,200,sent.text);
    // Read the session's own transcript, not an answer from the project's checkout.
    await until(async ()=> {
      const got=await request(engine,"GET","api/session/"+session+"/message?limit=30");
      const messages=got.json?.data??[];
      return messages.some(row=>row.type==="assistant" && JSON.stringify(row).includes(token)) && messages.some(row=>row.type==="idle" && row.outcome==="succeeded");
    },"the model to report the checkout-only note",100000,1000);
    assert.equal(fs.existsSync(path.join(project,"checkout-note.txt")),false);
    const archived=await call("POST","kuvo/sessions/"+session+"/archive",{archived:true});
    assert.equal(archived.status,200,archived.text);
    await until(()=> !fs.existsSync(worktree.root) && JSON.parse(fs.readFileSync(path.join(home,"box","worktrees.json"),"utf8")).restorable?.some(row=>row.root===worktree.root), "the archived worktree to be removed",20000);
    const restored=await call("POST","kuvo/worktrees/restore",{folder});
    assert.equal(restored.status,200,restored.text);
    assert.equal(fs.readFileSync(path.join(folder,"checkout-note.txt"),"utf8"),token+"\n");
  } finally {
    await box?.stop();
    await worker?.stop();
    await relay.close();
    fs.rmSync(root,{recursive:true,force:true,maxRetries:5});
  }
});
