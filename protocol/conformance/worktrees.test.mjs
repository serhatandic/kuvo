// Worktree location previews use real Git, with a deterministic engine inventory.
import "./routes.test.mjs";
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, isolatedEnv, openLine, request, until, write } from "./adapter.mjs";
import { bench } from "./bench.mjs";
import { git } from "./engine.mjs";

test("worktree locations use the main checkout, distinguish projects and never create folders", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("worktree-locations");
  const env = isolatedEnv(path.dirname(here.home));
  const shop = path.join(here.home, "shop");
  const nested = path.join(shop, "web");
  const other = path.join(here.home, "elsewhere", "web");
  const plain = path.join(here.home, "plain");
  const unborn = path.join(here.home, "unborn");
  for (const folder of [nested, other, plain, unborn]) fs.mkdirSync(folder, { recursive: true });
  for (const folder of [shop, other, unborn]) git(env, folder, "init", "-q", "-b", "main");
  for (const folder of [shop, other]) git(env, folder, "commit", "-qm", "First", "--allow-empty");
  write(path.join(shop, ".gitignore"), ".worktrees/\n");
  write(path.join(here.home, "box", "projects.json"), JSON.stringify({ projects: [nested, other, plain, unborn].map(directory => ({ directory })) }));
  here.engine.projects.set("123456abcdef", shop);
  const record = await here.start().ready();
  const where = async directory => {
    const route = "kuvo/worktrees/where" + (directory === undefined ? "" : "?directory=" + encodeURIComponent(directory));
    const answer = await request(record, "GET", route);
    assert.equal(answer.status, 200, answer.text);
    return answer.json;
  };
  const defaults = await where();
  assert.equal(defaults.directory, null);
  assert.equal(defaults.default, path.join(here.home, "data", "opencode", "worktree"));
  assert.deepEqual(defaults.projects, [
    { project: nested, name: "web — shop", folder: path.join(defaults.default, "123456"), exists: false, canMake: true },
    { project: other, name: "web — elsewhere", folder: null },
  ]);
  const relative = await where(".worktrees");
  assert.deepEqual(relative.projects.map(({ folder, inside, ignored }) => ({ folder, inside, ignored })), [
    { folder: path.join(shop, ".worktrees"), inside: true, ignored: true },
    { folder: path.join(other, ".worktrees"), inside: true, ignored: false },
  ]);
  const absolute = await where("~/Worktrees");
  assert.equal(absolute.projects[0].folder, path.join(defaults.home, "Worktrees"));
  assert.equal(absolute.projects[0].inside, undefined);
  assert.equal(fs.existsSync(absolute.projects[0].folder), false);
  const blocked = path.join(here.home, "file");
  write(blocked, "not a folder");
  assert.equal((await where(path.join(blocked, "trees"))).projects[0].canMake, false);
  for (const traversal of ["../trees","..\\trees"]) assert.equal((await request(record,"GET","kuvo/worktrees/where?directory="+encodeURIComponent(traversal))).status,403);
});

test("archiving the last session saves its edits and restoring puts them back", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("worktree-archive");
  const env = isolatedEnv(path.dirname(here.home));
  const project = path.join(here.home, "project");
  write(path.join(project, "file.txt"), "Committed\n");
  git(env, project, "init", "-q", "-b", "main");
  git(env, project, "add", "file.txt");
  git(env, project, "commit", "-qm", "First");
  write(path.join(here.home, "box", "projects.json"), JSON.stringify({ projects: [{ directory: project, name: "Fixture" }] }));
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow conversations");
  const started = await request(record, "POST", "kuvo/sessions", { project: "Fixture", worktree: true, title: "Worktree edits" });
  assert.equal(started.status, 200, started.text);
  const { session, worktree } = started.json;
  assert.equal(worktree.folder, worktree.root);
  const records = () => JSON.parse(fs.readFileSync(path.join(here.home, "box", "worktrees.json"), "utf8"));
  await until(() => records().worktrees.some(tree => tree.root === worktree.root && tree.used), "the worktree to be marked used");
  await until(() => git(env, worktree.root, "symbolic-ref", "--short", "HEAD") === "kuvo/worktree-edits", "the worktree branch to be named after the session");
  write(path.join(worktree.root, "file.txt"), "Edited\n");
  write(path.join(worktree.root, "new.txt"), "Untracked\n");
  const archived = await request(record, "POST", "kuvo/sessions/" + session + "/archive", { archived: true });
  assert.equal(archived.status, 200, archived.text);
  here.engine.emit({ type: "session.metadata.updated", data: { sessionID: session, metadata: { "kuvo.archived": true } } });
  await until(() => !fs.existsSync(worktree.root) && records().restorable?.some(tree => tree.root === worktree.root && tree.snapshot), "the archived checkout to be kept for restoration");
  const restored = await request(record, "POST", "kuvo/worktrees/restore", { folder: worktree.folder });
  assert.equal(restored.status, 200, restored.text);
  assert.equal(restored.json.used,false);
  assert.equal(restored.json.restored,true);
  for (const field of ["head","snapshot","made","prefix"]) assert.equal(restored.json[field],undefined,field);
  assert.equal(fs.readFileSync(path.join(worktree.root, "file.txt"), "utf8"), "Edited\n");
  assert.equal(fs.readFileSync(path.join(worktree.root, "new.txt"), "utf8"), "Untracked\n");
  assert.equal(git(env, worktree.root, "diff", "--cached", "--name-only"), "");
  assert.equal(fs.readFileSync(path.join(project, "file.txt"), "utf8"), "Committed\n");
});

test("the first worktree leaves staged files alone, followers see it and its last deleted session takes it away", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("first-worktree");
  const env = isolatedEnv(path.dirname(here.home));
  const project = path.join(here.home, "project");
  write(path.join(project, "staged.txt"), "Staged before first commit\n");
  git(env, project, "init", "-q", "-b", "main");
  git(env, project, "add", "staged.txt");
  const record = await here.start().ready();
  await until(() => here.engine.subscribers() > 0, "the resident to follow conversations");
  const line = await openLine(record, "", "kuvo/worktrees/events", {}, 30000);
  try {
    assert.equal(line.status, 200);
    const frames = line.frames();
    const first = (await frames.next()).value;
    assert.deepEqual(first.worktrees, []);
    const made = await request(record, "POST", "kuvo/worktrees", { project });
    assert.equal(made.status, 200, made.text);
    const tree = made.json;
    assert.equal(tree.folder,tree.root);
    assert.equal(git(env, project, "diff", "--cached", "--name-only"), "staged.txt");
    assert.equal(git(env, project, "ls-tree", "--name-only", "HEAD"), "");
    assert.equal(fs.existsSync(path.join(tree.root, "staged.txt")), false);
    let visible;
    do { visible = (await frames.next()).value; } while (!visible?.worktrees.some(row => row.root === tree.root));
    const session = await request(record, "POST", "api/session", { location: { directory: tree.folder } });
    assert.equal(session.status, 200, session.text);
    const id = session.json.data.id;
    const records = () => JSON.parse(fs.readFileSync(path.join(here.home, "box", "worktrees.json"), "utf8"));
    await until(() => records().worktrees.some(row => row.root === tree.root && row.used), "the worktree to be used");
    write(path.join(tree.root, "draft.txt"), "No session will use this\n");
    const removed = await request(record, "DELETE", "api/session/" + id);
    assert.equal(removed.status, 200, removed.text);
    await until(() => !fs.existsSync(tree.root) && records().worktrees.length === 0 && records().restorable.length === 0, "the deleted session's worktree to be removed");
    assert.equal(git(env, project, "branch", "--list", "kuvo/*"), "");
    assert.equal(git(env, project, "for-each-ref", "refs/kuvo/worktrees"), "");
    do { visible = (await frames.next()).value; } while (visible?.worktrees.some(row => row.root === tree.root));
    assert.deepEqual(visible.worktrees, []);
    assert.equal(fs.readFileSync(path.join(project, "staged.txt"), "utf8"), "Staged before first commit\n");
  } finally { line.close(); }
});

test("worktree records written by a newer version are refused without changes", async t => {
  const { computer } = await bench(t);
  const here = await computer("future-worktree-records");
  const file = path.join(here.home, "box", "worktrees.json");
  const future = '{"formatVersion":2,"future":["keep"]}';
  write(file, future);
  const record = await here.start().ready();
  for (const [method, route, body] of [
    ["GET", "kuvo/worktrees"],
    ["POST", "kuvo/worktrees/record", { removed: true, project: path.join(here.home,"project"), folder: path.join(here.home,"gone") }],
  ]) {
    const response = await request(record, method, route, body);
    assert.equal(response.status, 409, response.text);
    assert.match(response.json.message, /Worktree records format version 2/);
    assert.equal(fs.readFileSync(file, "utf8"), future);
  }
});

test("a title heard after restart names an unpushed branch once, avoiding names in use", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("worktree-branch-names");
  const env = isolatedEnv(path.dirname(here.home));
  const project = path.join(here.home, "project");
  const tree = path.join(here.home, "rename");
  const pushed = path.join(here.home, "pushed");
  write(path.join(project, "file"), "First\n");
  git(env, project, "init", "-q", "-b", "main");
  git(env, project, "add", "-A");
  git(env, project, "commit", "-qm", "First");
  git(env, project, "branch", "kuvo/ispravit-vhod");
  git(env, project, "worktree", "add", "-qb", "kuvo/initial", tree);
  git(env, project, "worktree", "add", "-qb", "kuvo/previously-pushed", pushed);
  git(env, project, "update-ref", "refs/remotes/origin/kuvo/previously-pushed", "HEAD");
  const recordFile = path.join(here.home, "box", "worktrees.json");
  write(recordFile, JSON.stringify({ worktrees: [ [tree,"kuvo/initial"], [pushed,"kuvo/previously-pushed"] ].map(([root,branch]) => ({ project, root, folder:root, branch, base:"main", made:branch, prefix:"kuvo/" })) }));
  const { sessionOf } = await import("./engine.mjs");
  const first = sessionOf({ id:"ses_treeTitle", directory:tree, title:"Исправить вход" });
  const second = sessionOf({ id:"ses_pushedTitle", directory:pushed, title:"Another title" });
  here.engine.sessions.push(first, second);
  const record = await here.start().ready();
  await until(() => {
    const saved = JSON.parse(fs.readFileSync(recordFile,"utf8")).worktrees;
    return saved.find(row => row.root === tree)?.branch === "kuvo/ispravit-vhod-2";
  }, "the saved title to rename the unpushed branch");
  assert.equal(git(env, tree, "symbolic-ref", "--short", "HEAD"), "kuvo/ispravit-vhod-2");
  assert.equal(git(env, pushed, "symbolic-ref", "--short", "HEAD"), "kuvo/previously-pushed");
  const renamed = await request(record,"POST","kuvo/sessions/"+first.id+"/rename",{title:"Another name"});
  assert.equal(renamed.status,200,renamed.text);
  here.engine.emit({type:"session.renamed",data:{sessionID:first.id}});
  const records = await request(record,"GET","kuvo/worktrees?records=1");
  assert.equal(records.status,200,records.text);
  assert.equal(records.json.worktrees.find(row => row.root === tree).branch,"kuvo/ispravit-vhod-2");
  assert.equal(git(env, tree, "symbolic-ref", "--short", "HEAD"), "kuvo/ispravit-vhod-2");
});

test("removing and restoring a checkout preserves a commit on detached HEAD", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("worktree-detached");
  const env = isolatedEnv(path.dirname(here.home));
  const project = path.join(here.home, "project");
  write(path.join(project,"file"),"First\n");
  git(env,project,"init","-q","-b","main");
  git(env,project,"add","-A");
  git(env,project,"commit","-qm","First");
  const record = await here.start().ready();
  const made = await request(record,"POST","kuvo/worktrees",{project});
  assert.equal(made.status,200,made.text);
  const {root,folder} = made.json;
  const session = await request(record,"POST","api/session",{location:{directory:folder}});
  assert.equal(session.status,200,session.text);
  await until(() => JSON.parse(fs.readFileSync(path.join(here.home,"box","worktrees.json"),"utf8")).worktrees.some(tree => tree.root===root && tree.used), "session to use checkout");
  git(env,root,"checkout","--detach","-q");
  write(path.join(root,"file"),"Detached edit\n");
  git(env,root,"commit","-qam","Detached commit");
  const commit = git(env,root,"rev-parse","HEAD");
  const remove = await request(record,"POST","kuvo/worktrees/remove",{path:root});
  assert.equal(remove.status,200,remove.text);
  assert.equal(fs.existsSync(root),false);
  assert.match(git(env,project,"branch","--contains",commit),/-kept/);
  const restored = await request(record,"POST","kuvo/worktrees/restore",{folder});
  assert.equal(restored.status,200,restored.text);
  assert.equal(git(env,root,"rev-parse","HEAD"),commit);
  assert.equal(fs.readFileSync(path.join(root,"file"),"utf8"),"Detached edit\n");
});

test("project removal archives its filed conversations and keeps the project on a refused archive", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here = await computer("archive-project");
  const project = path.join(here.home,"project");
  const other = path.join(here.home,"other-folder");
  const oldTree = path.join(here.home,"removed-tree");
  const filedFolder = path.join(here.home,"filed");
  const unrelated = path.join(here.home,"unrelated");
  write(path.join(here.home,"box","projects.json"),JSON.stringify({projects:[{directory:project,name:"Project",folders:[other]}]}));
  write(path.join(here.home,"box","worktrees.json"),JSON.stringify({removed:{[oldTree]:project}}));
  const { sessionOf } = await import("./engine.mjs");
  for (const [id,directory] of [["ses_project",project],["ses_otherFolder",other],["ses_removedTree",oldTree],["ses_filed",filedFolder],["ses_unrelated",unrelated]]) {
    const row=sessionOf({id,directory});
    if (id==="ses_filed") row.metadata={kuvo:{filed:{folder:filedFolder,project}}};
    here.engine.sessions.push(row);
  }
  const record=await here.start().ready();
  const route="kuvo/projects?directory="+encodeURIComponent(project);
  here.engine.holdSessionPatch={id:"ses_removedTree",accepted(){},release:Promise.resolve(),status:503};
  const refused=await request(record,"DELETE",route);
  assert.equal(refused.status,409,refused.text);
  assert.equal(refused.json.archived,3);
  assert.equal(refused.json.kept,1);
  assert.deepEqual(refused.json.sessions.toSorted(),["ses_filed","ses_otherFolder","ses_project"]);
  assert.equal((await request(record,"GET","kuvo/projects")).json.projects[0].removed,undefined);
  here.engine.holdSessionPatch=null;
  const retry=await request(record,"DELETE",route);
  assert.equal(retry.status,200,retry.text);
  assert.deepEqual(retry.json,{archived:1,sessions:["ses_removedTree"]});
  assert.equal((await request(record,"GET","kuvo/projects")).json.projects[0].removed,true);
  assert.equal(here.engine.sessions.find(row=>row.id==="ses_unrelated").metadata?.["kuvo.archived"],undefined);
});

test("a shared checkout stays for its open or working sessions and when the run list cannot be read", { timeout: 45000 }, async t => {
  const { computer } = await bench(t);
  const here=await computer("shared-worktree");
  const env=isolatedEnv(path.dirname(here.home));
  const project=path.join(here.home,"project");
  write(path.join(project,"file"),"First\n");
  git(env,project,"init","-q","-b","main");
  git(env,project,"add","-A");
  git(env,project,"commit","-qm","First");
  const record=await here.start().ready();
  await until(()=>here.engine.subscribers()>0,"the engine event follower");
  const made=await request(record,"POST","kuvo/worktrees",{project});
  assert.equal(made.status,200,made.text);
  const {root,folder}=made.json;
  const create=async title=>{
    const answer=await request(record,"POST","api/session",{title,location:{directory:folder}});
    assert.equal(answer.status,200,answer.text);
    return answer.json.data.id;
  };
  const first=await create("First session");
  const second=await create("Second session");
  const records=()=>JSON.parse(fs.readFileSync(path.join(here.home,"box","worktrees.json"),"utf8"));
  await until(()=>records().worktrees.some(row=>row.root===root&&row.used),"the checkout to be used");
  const archive=async id=>{
    const response=await request(record,"POST","kuvo/sessions/"+id+"/archive",{archived:true});
    assert.equal(response.status,200,response.text);
    here.engine.emit({type:"session.metadata.updated",data:{sessionID:id,metadata:{"kuvo.archived":true}}});
  };
  await archive(first);
  assert.equal(fs.statSync(root).isDirectory(),true);
  fs.writeFileSync(path.join(root,"draft.txt"),"Second session's edit\n");
  here.engine.running.add(second);
  await archive(second);
  const busy=await request(record,"POST","kuvo/worktrees/remove",{path:root});
  assert.equal(busy.status,409,busy.text);
  here.engine.running.delete(second);
  here.engine.refuseActive=true;
  const unknown=await request(record,"POST","kuvo/worktrees/remove",{path:root});
  assert.equal(unknown.status,503,unknown.text);
  assert.equal(fs.readFileSync(path.join(root,"draft.txt"),"utf8"),"Second session's edit\n");
  here.engine.refuseActive=false;
  here.engine.emit({type:"session.execution.succeeded",data:{sessionID:second}});
  await until(()=>!fs.existsSync(root)&&records().restorable.some(row=>row.root===root&&row.snapshot),"the last turn to end before keeping the checkout");
});

test("a saved checkout keeps the contents through the repository's normal clean and smudge filters", { timeout: 45000 }, async t => {
  const { computer }=await bench(t);
  const here=await computer("worktree-filter");
  const env=isolatedEnv(path.dirname(here.home));
  const project=path.join(here.home,"project");
  write(path.join(project,"file.txt"),"initial\n");
  write(path.join(project,".gitattributes"),"file.txt filter=letters\n");
  git(env,project,"init","-q","-b","main");
  git(env,project,"config","filter.letters.clean","tr a-z A-Z");
  git(env,project,"config","filter.letters.smudge","tr A-Z a-z");
  git(env,project,"config","filter.letters.required","true");
  git(env,project,"add","-A");
  git(env,project,"commit","-qm","First");
  const record=await here.start().ready();
  await until(()=>here.engine.subscribers()>0,"the engine event follower");
  const made=await request(record,"POST","kuvo/worktrees",{project});
  assert.equal(made.status,200,made.text);
  const {root,folder}=made.json;
  const started=await request(record,"POST","api/session",{location:{directory:folder}});
  assert.equal(started.status,200,started.text);
  const id=started.json.data.id;
  const records=()=>JSON.parse(fs.readFileSync(path.join(here.home,"box","worktrees.json"),"utf8"));
  await until(()=>records().worktrees.some(row=>row.root===root&&row.used),"the checkout to be used");
  fs.writeFileSync(path.join(root,"file.txt"),"edited\n");
  const archived=await request(record,"POST","kuvo/sessions/"+id+"/archive",{archived:true});
  assert.equal(archived.status,200,archived.text);
  here.engine.emit({type:"session.metadata.updated",data:{sessionID:id,metadata:{"kuvo.archived":true}}});
  const saved=await until(()=>records().restorable?.find(row=>row.root===root&&row.snapshot),"the filtered snapshot to be saved");
  assert.equal(git(env,project,"show",saved.snapshot+":file.txt"),"EDITED");
  const restored=await request(record,"POST","kuvo/worktrees/restore",{folder});
  assert.equal(restored.status,200,restored.text);
  assert.equal(fs.readFileSync(path.join(root,"file.txt"),"utf8"),"edited\n");
});

test("a retained coding-agent conversation names and keeps its checkout, and the engine conversation hears the name", { timeout: 60000 }, async t => {
  const { boxBinary, readJSON }=await import("./adapter.mjs");
  const { claudeEnvironment, seedSession, startClaudeBridge, stopFixtureHolders }=await import("./claude.mjs");
  const { sessionOf }=await import("./engine.mjs");
  const binary=boxBinary();
  const { computer }=await bench(t);
  const here=await computer("native-worktree",binary,folder=>{
    const fixture=claudeEnvironment(binary,path.join(folder,"box"),isolatedEnv(folder),{turns:true});
    return {...fixture, PATH:fixture.PATH+path.delimiter+process.env.PATH};
  });
  const env=isolatedEnv(path.dirname(here.home));
  const project=path.join(here.home,"project");
  const root=path.join(here.home,"tree");
  write(path.join(project,"file"),"First\n");
  git(env,project,"init","-q","-b","main");
  git(env,project,"add","-A");
  git(env,project,"commit","-qm","First");
  git(env,project,"worktree","add","-qb","kuvo/initial",root);
  write(path.join(here.home,"box","worktrees.json"),JSON.stringify({worktrees:[{project,root,folder:root,branch:"kuvo/initial",made:"kuvo/initial",prefix:"kuvo/",base:"main"}]}));
  const native="ses_claudeWorktree";
  seedSession(here.home,native,{directory:root});
  const engineSession=sessionOf({id:"ses_engineWorktree",title:"",directory:root});
  here.engine.sessions.push(engineSession);
  const bridge=await startClaudeBridge(binary,here.home,here.env,{engine:here.engine.record});
  let box;
  try {
    box=here.start();
    const record=await box.ready();
    const branch="kuvo/claude-fixture";
    await until(()=>git(env,root,"symbolic-ref","--short","HEAD")===branch,"the native conversation to name the checkout",20000);
    const entry="/api/experimental/session/"+engineSession.id+"/instructions/entries/kuvo.worktree";
    await until(()=>here.engine.asked.some(row=>row.method==="PUT"&&row.path===entry&&row.json.value.includes(branch)),"the engine conversation to hear its branch");
    const agentFile=path.join(here.home,"agents","sessions",native,"agent.json");
    await until(()=>fs.existsSync(agentFile)&&readJSON(agentFile).kuvoInstructions?.["kuvo.worktree"]?.includes(branch),"the native conversation to hear its branch");
    const listed=await request(record,"GET","kuvo/worktrees");
    assert.equal(listed.status,200,listed.text);
    assert.equal(listed.json.worktrees.find(row=>row.path===root).sessions,2);
    const removed=await request(record,"DELETE","api/session/"+engineSession.id);
    assert.equal(removed.status,200,removed.text);
    const forget=await request(record,"POST","kuvo/worktrees/remove",{path:root,forget:true});
    assert.equal(forget.status,409,forget.text);
    assert.equal(fs.existsSync(root),true);
  } finally {
    await box?.stop();
    await bridge?.stop();
    await stopFixtureHolders(here.home);
  }
});

test("a stopped engine still previews a chosen folder for an existing repository on an unborn branch", async t=>{
  const {computer}=await bench(t);
  const here=await computer("worktree-where-parked");
  const env=isolatedEnv(path.dirname(here.home));
  const project=path.join(here.home,"project");
  fs.mkdirSync(project);
  git(env,project,"init","-q","-b","main");
  git(env,project,"commit","-qm","First","--allow-empty");
  git(env,project,"checkout","-q","--orphan","other");
  write(path.join(here.home,"box","projects.json"),JSON.stringify({projects:[{directory:project}]}));
  // A parked engine: the fixture process is still owned by the test, not known to the resident.
  fs.unlinkSync(path.join(here.home,"run","engine.json"));
  const record=await here.start().ready();
  const defaults=await request(record,"GET","kuvo/worktrees/where");
  assert.equal(defaults.status,200,defaults.text);
  assert.deepEqual(defaults.json.projects,[{project,name:"project",folder:null}]);
  const named=await request(record,"GET","kuvo/worktrees/where?directory="+encodeURIComponent(path.join(here.home,"trees")));
  assert.equal(named.status,200,named.text);
  assert.deepEqual(named.json.projects,[{project,name:"project",folder:path.join(here.home,"trees"),exists:false,canMake:true}]);
  assert.equal(fs.existsSync(path.join(here.home,"run","engine.json")),false);
});

test("an old Mac record is taken over once and then follows the checkout's kept branch", async t=>{
  const {computer}=await bench(t);
  const here=await computer("worktree-old-record");
  const env=isolatedEnv(path.dirname(here.home));
  const project=path.join(here.home,"project");
  const root=path.join(here.home,"tree");
  fs.mkdirSync(project);
  git(env,project,"init","-q","-b","main");
  git(env,project,"commit","-qm","First","--allow-empty");
  git(env,project,"worktree","add","-qb","old-branch",root);
  const record=await here.start().ready();
  const given={project,root,branch:"old-branch",base:"main"};
  assert.equal((await request(record,"POST","kuvo/worktrees/record",given)).status,200);
  assert.equal((await request(record,"POST","kuvo/worktrees/record",{...given,branch:"other"})).status,200);
  const listed=await request(record,"GET","kuvo/worktrees?records=1");
  assert.equal(listed.status,200,listed.text);
  assert.deepEqual(listed.json.worktrees,[{...given,folder:root}]);
  const gone=path.join(here.home,"gone");
  assert.equal((await request(record,"POST","kuvo/worktrees/record",{removed:true,folder:gone,project})).status,200);
  const projects=await request(record,"GET","kuvo/projects");
  assert.equal(projects.status,200,projects.text);
  assert.equal(projects.json.worktrees[gone],project);
});

test("Settings shows worktree locations and refuses a stale removal confirmation", {timeout:45000}, async t=>{
  const {computer}=await bench(t);
  const here=await computer("worktree-settings");
  const env=isolatedEnv(path.dirname(here.home));
  const project=path.join(here.home,"project");
  write(path.join(project,"file"),"Initial\n");
  git(env,project,"init","-q","-b","main");
  git(env,project,"add","-A");
  git(env,project,"commit","-qm","First");
  write(path.join(here.home,"box","opencode.json"),"{}\n");
  write(path.join(here.home,"box","projects.json"),JSON.stringify({projects:[{directory:project}]}));
  const record=await here.start().ready();
  const ask=here.connect(record);
  const page=async()=>{
    const result=await ask("GET","kuvo/settings/worktrees");
    assert.equal(result.status,200,result.text);
    return result.json;
  };
  const row=(shown,id)=>shown.groups.flatMap(g=>g.rows.flatMap(r=>[r,...(r.items??[])])).find(r=>r.id===id);
  const initial=await page();
  assert.equal(row(initial,"worktrees-folder").value,"");
  assert.deepEqual(row(initial,"worktrees").items,[]);
  const selected=path.join(project,".trees");
  const field=row(initial,"worktrees-folder");
  const changed=await ask("PATCH","kuvo/settings/worktrees",{row:field.id,was:field.revision,value:selected});
  assert.equal(changed.status,200,changed.text);
  assert.equal(row(changed.json,"worktrees-folder").value,selected);
  assert.equal(row(changed.json,"place-"+project).tone,"warning");
  const made=await ask("POST","kuvo/worktrees",{project});
  assert.equal(made.status,200,made.text);
  const root=made.json.root;
  const checkout=shown=>row(shown,"worktrees").items.find(item=>item.value?.record?.root===root);
  const shown=await until(async()=> {
    const item=checkout(await page());
    return item?.value?.uncommitted===false&&item;
  },"the checkout's current clean status to be shown",15000);
  assert.equal(shown.actions[0].can,true);
  fs.writeFileSync(path.join(root,"draft"),"Changed after confirmation\n");
  const action=at=>ask("POST","kuvo/settings/worktrees/"+encodeURIComponent(at.id)+"/remove",{was:at.revision});
  // The frozen reference's worktree row ID has raw path separators and cannot pass
  // its own route path guard. Exercise its equivalent owner action instead.
  const reference=boxBinary().typescript;
  if (reference) assert.equal((await action(shown)).status,403);
  const remove=at=>reference ? ask("POST","kuvo/worktrees/remove",{path:root,was:at.value}) : action(at);
  const stale=await remove(shown);
  assert.equal(stale.status,409,stale.text);
  assert.equal(fs.readFileSync(path.join(root,"draft"),"utf8"),"Changed after confirmation\n");
  const fresh=await until(async()=> {
    const item=checkout(await page());
    return item?.value?.uncommitted===true&&item;
  },"the new confirmation to include uncommitted work",15000);
  const removed=await remove(fresh);
  assert.equal(removed.status,200,removed.text);
  assert.equal(fs.existsSync(root),false);
  assert.deepEqual(row(reference?await page():removed.json,"worktrees").items,[]);
});
