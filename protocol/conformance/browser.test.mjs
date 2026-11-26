// Browser preferences cross the real resident route, with no browser process or real profile.
import assert from "node:assert/strict";
import { existsSync, readFileSync } from "node:fs";
import path from "node:path";
import { test } from "node:test";
import { bench } from "./bench.mjs";

test("browser settings keep independent preferences and validate edits", async t => {
  const { computer } = await bench(t);
  const box = await computer("Browser");
  const ask = box.connect(await box.start().ready());
  const initial = await ask("GET", "kuvo/browser/settings");
  assert.equal(initial.status, 200, initial.text);
  assert.deepEqual(initial.json, {
    searchEngine: "google", searchEngines: [
      {id:"google",title:"Google"}, {id:"duckDuckGo",title:"DuckDuckGo"},
      {id:"bing",title:"Bing"}, {id:"brave",title:"Brave Search"},
      {id:"kagi",title:"Kagi"}, {id:"ecosia",title:"Ecosia"},
    ], tidiesAgentTabs: true, offersToSave: true, never: [],
  });
  const changes = {searchEngine:"ecosia", tidiesAgentTabs:false, offersToSave:false,
    never:["https://Bank.example.com/login", "bank.example.com"]};
  const saved = await ask("PATCH", "kuvo/browser/settings", changes);
  assert.equal(saved.status, 200, saved.text);
  assert.deepEqual(saved.json, {...initial.json, ...changes, never:["bank.example.com"]});
  const folder = path.join(box.home,"browser");
  assert.deepEqual(JSON.parse(readFileSync(path.join(folder,"settings.json"),"utf8")),
    {searchEngine:"ecosia",offersToSave:false,never:["bank.example.com"]});
  assert.ok(existsSync(path.join(folder,"keeps-agent-tabs")));
  for (const [invalid, message] of [
    [{searchEngine:"altavista"},"The search engine is one of google, duckDuckGo, bing, brave, kagi, ecosia."],
    [{color:"blue"}, 'There\'s no browser setting "color".'],
    [{offersToSave:"false"},"offersToSave is true or false."],
    [{never:["bad host"]},"never is a list of sites, each a host (example.com)."],
  ]) {
    const answer = await ask("PATCH","kuvo/browser/settings",invalid);
    assert.equal(answer.status,400,answer.text);
    assert.equal(answer.json.message,message);
  }
  assert.deepEqual((await ask("GET","kuvo/browser/settings")).json,saved.json);
  const added = await ask("PUT","kuvo/browser/settings/never/new.example");
  assert.equal(added.status,200,added.text);
  assert.deepEqual(added.json.never,["bank.example.com","new.example"]);
  const removed = await ask("DELETE","kuvo/browser/settings/never/bank.example.com");
  assert.equal(removed.status,200,removed.text);
  assert.deepEqual(removed.json.never,["new.example"]);
  const tidy = await ask("PATCH","kuvo/browser/settings",{tidiesAgentTabs:true});
  assert.equal(tidy.status,200,tidy.text);
  assert.deepEqual(tidy.json,{...removed.json,tidiesAgentTabs:true});
  assert.ok(!existsSync(path.join(folder,"keeps-agent-tabs")));
});

test("browser history folds visits and keeps a site's icon until forgotten", async t => {
  const { computer } = await bench(t);
  const box = await computer("History");
  const folder = path.join(box.home,"browser");
  const { write } = await import("./adapter.mjs");
  const now = Date.now();
  const png = Buffer.from("89504e470d0a1a0a0000000d49484452000000010000000108060000001f15c489", "hex");
  const visits = [
    {url:"https://www.example.org/guide",title:"Guide",at:now-3000},
    {url:"https://example.org/",title:"Example",at:now-2000},
    {url:"https://example.org/another",title:"Another",at:now-1000},
    {url:"https://example.org/",icon:png.toString("base64"),at:now},
    {url:"about:blank",title:"Blank",at:now},
    {url:"http://localhost:5173/",title:"http://localhost:5173/",at:now},
  ];
  write(path.join(folder,"visits.jsonl"),visits.map(v=>JSON.stringify(v)).join("\n")+"\n");
  const ask = box.connect(await box.start().ready());
  const history = await ask("GET","kuvo/browser/history");
  assert.equal(history.status,200,history.text);
  assert.deepEqual(history.json,{sites:[
    {host:"example.org",url:"https://example.org/",title:"Example",visits:3,lastVisit:now-1000,icon:true},
    {host:"localhost:5173",url:"http://localhost:5173/",title:"",visits:1,lastVisit:now,icon:false},
  ]});
  assert.ok(!existsSync(path.join(folder,"visits.jsonl")));
  assert.deepEqual((await ask("GET","kuvo/browser/history")).json,history.json);
  const icon = await ask("GET","kuvo/browser/icons/example.org");
  assert.equal(icon.status,200,icon.text);
  assert.deepEqual(readFileSync(path.join(folder,"icons","example.org")),png);
  const forget = await ask("DELETE","kuvo/browser/history/example.org");
  assert.equal(forget.status,200,forget.text);
  assert.deepEqual(forget.json,{sites:[history.json.sites[1]]});
  assert.equal((await ask("GET","kuvo/browser/icons/example.org")).status,404);
  const clear = await ask("DELETE","kuvo/browser/history");
  assert.equal(clear.status,200,clear.text);
  assert.deepEqual(clear.json,{sites:[]});
  assert.ok(!existsSync(path.join(folder,"history.json")));
});

test("show tools say what they showed without opening the browser", async t => {
  const { computer } = await bench(t);
  const box = await computer("Show");
  const { write } = await import("./adapter.mjs");
  const { sessionOf } = await import("./engine.mjs");
  const folder = path.join(box.home,"work");
  const file = path.join(folder,"report.txt");
  write(file,"a report");
  box.engine.sessions.push(sessionOf({id:"ses_show",title:"Show",directory:folder}));
  const ask = box.connect(await box.start().ready());
  async function tool(name,args,inView) {
    const reply = await ask("POST","kuvo/agent-tools",{name,arguments:args,session:"ses_show",inView});
    assert.equal(reply.status,200,reply.text);
    return reply.json;
  }
  assert.deepEqual(await tool("show_file",{path:file}),{text:`Shown in the conversation: the file ${file}. The user sees it there, on their Mac and their phone.`,isError:false});
  assert.deepEqual(await tool("show_file",{path:folder}),{text:`${folder} is a folder: show a file in it, or the folder with show_in_panel.`,isError:true});
  assert.deepEqual(await tool("show_in_panel",{path:"report.txt"}),{text:`Give the full path: ${file}`,isError:true});
  assert.deepEqual(await tool("show_in_panel",{path:file},true),{text:`Shown: the file ${file} is open in Kuvo Code's side panel, beside this conversation.`,isError:false});
  assert.deepEqual(await tool("show_in_panel",{path:folder},false),{text:`Kept for later: the folder ${folder} opens in Kuvo Code's side panel when the user comes back to this conversation. They're in another one right now, so don't say it's open.`,isError:false});
  assert.deepEqual(await tool("mark_final",{path:file}),{text:`Marked final: ${file}. It's kept at the top of what you made, for the user.`,isError:false});
  assert.deepEqual(await tool("show_widget",{html:"<p>Report</p>"}),{text:"Shown in the conversation, live. The user sees it now; what they click in it that calls kuvo.send comes to you as their message.",isError:false});
  assert.deepEqual(await tool("show_widget",{html:" "}),{text:"Missing html.",isError:true});
  assert.ok(!existsSync(path.join(box.home,"run","browser.pid")));
});

test("the tool server lists browser and show tools and answers an attributed show call", { timeout: 30000 }, async t => {
  const { computer } = await bench(t);
  const box = await computer("Tools");
  const { boxBinary, launch, write, typescriptRuntime } = await import("./adapter.mjs");
  const { sessionOf } = await import("./engine.mjs");
  const { createInterface } = await import("node:readline");
  const fs = await import("node:fs");
  const binary = boxBinary();
  const folder = path.join(box.home,"work");
  const file = path.join(folder,"report.txt");
  write(file,"a report");
  box.engine.sessions.push(sessionOf({id:"ses_show",title:"Show",directory:folder}));
  if (binary.typescript) {
    fs.mkdirSync(path.join(box.home,"bin"),{recursive:true});
    fs.copyFileSync(binary.file,path.join(box.home,"bin","kuvo-link.js"));
  }
  const resident = box.connect(await box.start().ready());
  assert.equal((await resident("GET","kuvo/info")).status,200);
  let client;
  if (binary.typescript) {
    const script = path.join(path.dirname(binary.file),"kuvo-browser.js");
    client = launch(typescriptRuntime(),["--no-env-file",script,"mcp"],{...box.env,KUVO_HOME:box.home,BUN_BE_BUN:"1"},box.home,"pipe");
  } else {
    const record = JSON.parse(readFileSync(path.join(box.home,"run","agents","relay.json"),"utf8"));
    client = launch(binary.file,["mcp"],{...box.env,KUVO_HOME:box.home,KUVO_TOOL_RELAY:`http://127.0.0.1:${record.port}/mcp`,KUVO_TOOL_RELAY_KEY:`Bearer ${record.keys["@engine"]}`,KUVO_TOOL_TIMEOUT:"960000"},box.home,"pipe");
  }
  t.after(()=>client.stop());
  const lines = createInterface({input:client.child.stdout})[Symbol.asyncIterator]();
  let id = 0;
  async function ask(method,params={}) {
    client.child.stdin.write(JSON.stringify({jsonrpc:"2.0",id:++id,method,params})+"\n");
    const answer = await lines.next();
    assert.ok(!answer.done,client.log());
    const message = JSON.parse(answer.value);
    assert.equal(message.id,id,answer.value);
    assert.ok(message.result,answer.value);
    return message.result;
  }
  const started = await ask("initialize",{protocolVersion:"2025-06-18"});
  assert.equal(started.serverInfo.name,"kuvo-browser");
  assert.equal(started.protocolVersion,"2025-06-18");
  const {tools} = await ask("tools/list");
  for (const name of ["browser_navigate","browser_snapshot","browser_click","browser_ask_code","show_file","show_in_panel","show_widget","mark_final","list_agents"]) {
    const found = tools.find(t=>t.name===name);
    assert.ok(found,`Missing ${name}`);
    assert.equal(found.inputSchema.type,"object");
  }
  const shown = await ask("tools/call",{name:"show_in_panel",arguments:{path:"report.txt"},_meta:{"ai.opencode/sessionID":"ses_show"}});
  assert.deepEqual(shown,{content:[{type:"text",text:`Give the full path: ${file}`}],isError:true});
  assert.ok(!existsSync(path.join(box.home,"run","browser.pid")));
  client.child.stdin.end();
  await client.exited;
});
