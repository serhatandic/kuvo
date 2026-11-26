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
  const { computer,relay } = await bench(t);
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
    client = launch(typescriptRuntime(),["--no-env-file",script,"mcp"],{...box.env,KUVO_HOME:box.home,KUVO_TEST_RELAY:relay.url,KUVO_RELAY_URL:relay.url,BUN_BE_BUN:"1"},box.home,"pipe");
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

// The same pinned source fixture as the TypeScript browser-install scenarios.
async function installation(t, answer = null) {
  const {makeZip,makeRpm,served,startSource}=await import("../../box/bridge-tests/browser-source-stand-in.js");
  const fs=await import("node:fs");
  const {boxBinary,write,until}=await import("./adapter.mjs");
  const arch=process.arch==="arm64"?"aarch64":"x86_64";
  const zip=makeZip([{name:"chrome-linux64/chrome",data:"#!/bin/sh\nexit 99\n",mode:0o755,deflate:true},{name:"chrome-linux64/LICENSE",data:"chrome"}]);
  const rpm=makeRpm([{name:"./usr/lib64/libexample.so.1.0",data:"library"},{name:"./usr/lib64/libexample.so.1",link:"libexample.so.1.0"},{name:"./usr/share/fonts/example/Example.ttf",data:"font"},{name:"./usr/share/licenses/example/COPYING",data:"license"},{name:"./usr/bin/example",data:"not needed"}]);
  const {pins,files}=served({linux:{version:"155.0.0.1",aarch64:zip,x86_64:zip,packages:{[arch]:[{name:"example",rpm}]}}});
  const source=answer?await answer(files):await startSource(files);
  t.after(()=>source.close());
  const {computer}=await bench(t);
  const box=await computer("Installer",undefined,folder=>{
    const pin=path.join(folder,"pins.json");write(pin,JSON.stringify(pins));
    return {KUVO_BROWSER_SOURCE:source.url,KUVO_BROWSER_PINS:pin,KUVO_BROWSER_SYSTEM:"linux"};
  });
  const ask=box.connect(await box.start().ready());
  // The reference starts this from the shell keeper before the resident; its
  // direct resident fixture requests it over the route instead.
  if(boxBinary().typescript) assert.equal((await ask("POST","kuvo/browser",{})).status,202);
  const state=async()=>{const answer=await ask("GET","kuvo/browser");assert.equal(answer.status,200,answer.text);return answer.json;};
  const ready=()=>until(async()=>{const status=await state();return status.state==="ready"&&status;},"The browser wasn't installed",30000);
  return {box,ask,state,ready,source,fs,zip,until};
}

test("the pinned browser is checked and prepared without running Chrome, only once",{timeout:60000, skip:process.platform==="win32"},async t=>{
  const {box,ask,ready,source,fs}=await installation(t);
  const status=await ready();
  assert.deepEqual(status,{state:"ready",chromium:"155.0.0.1",installed:"155.0.0.1",progress:null,message:null,retryAt:null});
  const browser=path.join(box.home,"browser"),build=fs.realpathSync(path.join(browser,"current"));
  assert.equal(fs.readFileSync(path.join(build,"lib","libexample.so.1.0"),"utf8"),"library");
  assert.equal(fs.readlinkSync(path.join(build,"lib","libexample.so.1")),"libexample.so.1.0");
  assert.equal(fs.readFileSync(path.join(build,"fonts","Example.ttf"),"utf8"),"font");
  assert.equal(fs.readFileSync(path.join(build,"licenses","example","COPYING"),"utf8"),"license");
  assert.ok(!fs.existsSync(path.join(build,"usr")));
  assert.ok(!fs.existsSync(path.join(browser,"profile")),"Chrome ran during installation");
  assert.ok(!fs.existsSync(path.join(box.home,"run","browser.pid")));
  await Promise.all([ask("POST","kuvo/browser",{}),ask("POST","kuvo/browser",{})]);
  assert.equal(source.asked.length,2,"Only the pinned zip and package");
  assert.equal(fs.realpathSync(path.join(browser,"current")),build);
});

test("a different checksum is refused before the browser is unpacked",{timeout:60000, skip:process.platform==="win32"},async t=>{
  const {startSource}=await import("../../box/bridge-tests/browser-source-stand-in.js");
  const {box,state,fs,until}=await installation(t,files=>startSource(files,address=>{
    if(!address.endsWith(".zip"))return null;
    const corrupted=Buffer.from(files.get(address));corrupted[0]^=1;
    return {status:200,body:corrupted};
  }));
  const failed=await until(async()=>{const status=await state();return status.state==="failed"&&status;},"The changed archive wasn't refused",30000);
  assert.match(failed.message,/checksum/);
  assert.ok(failed.retryAt>Date.now());
  assert.ok(!fs.existsSync(path.join(box.home,"browser","current")));
  assert.ok(!fs.existsSync(path.join(box.home,"browser","profile")));
});

test("an interrupted download can be asked for again and is published once",{timeout:60000, skip:process.platform==="win32"},async t=>{
  const http=await import("node:http");
  const {once}=await import("node:events");
  const {boxBinary}=await import("./adapter.mjs");
  let ranges=[],zipRequests=0, interrupt;
  const {box,ask,state,ready,fs,until}=await installation(t,async files=>{
    const server=http.createServer((request,response)=>{
      const address=`https://${request.url.slice(1)}`;
      const body=files.get(address);
      if(!body){response.writeHead(404);response.end();return;}
      if(address.endsWith(".zip")){
        zipRequests++;
        if(zipRequests===1){
          response.writeHead(200,{"content-length":body.length});
          response.flushHeaders();
          response.write(body.subarray(0,Math.floor(body.length/2)));
          interrupt=()=>response.destroy();
          return;
        }
        if(request.headers.range){
          ranges.push(request.headers.range);
          const from=Number(/^bytes=(\d+)-$/.exec(request.headers.range)?.[1]);
          response.writeHead(206,{"content-length":body.length-from,"content-range":`bytes ${from}-${body.length-1}/${body.length}`});
          response.end(body.subarray(from));return;
        }
      }
      response.writeHead(200,{"content-length":body.length});response.end(body);
    });
    server.listen(0,"127.0.0.1");await once(server,"listening");
    return {url:`http://127.0.0.1:${server.address().port}`,close:()=>new Promise(resolve=>server.close(resolve))};
  });
  await until(async()=>{const status=await state();return interrupt && status.state==="downloading" && status.progress>0;},"The first part wasn't downloaded",30000);
  await Promise.all([ask("POST","kuvo/browser",{}),ask("POST","kuvo/browser",{})]);
  assert.equal(zipRequests,1,"One setup while a transfer is in progress");
  interrupt();
  await until(async()=>{const status=await state();return status.state==="failed"&&status;},"The partial download wasn't reported",30000);
  assert.equal((await ask("POST","kuvo/browser",{now:true})).status,202);
  assert.equal((await ready()).installed,"155.0.0.1");
  assert.equal(zipRequests,2);
  if(!boxBinary().typescript) assert.equal(ranges.length,1,"The second transfer resumed instead of starting over");
  assert.ok(fs.existsSync(path.join(box.home,"browser","current","chrome")));
});

async function browserMcp(box, record, relay) {
  const { boxBinary, launch, typescriptRuntime } = await import("./adapter.mjs");
  const { createInterface } = await import("node:readline");
  const fs = await import("node:fs");
  const binary=boxBinary();
  let client;
  if(binary.typescript){
    fs.mkdirSync(path.join(box.home,"bin"),{recursive:true});
    fs.copyFileSync(binary.file,path.join(box.home,"bin","kuvo-link.js"));
    const browser=path.join(path.dirname(binary.file),"kuvo-browser.js");
    fs.copyFileSync(browser,path.join(box.home,"bin","kuvo-browser.js"));
    client=launch(typescriptRuntime(),["--no-env-file",browser,"mcp"],{...box.env,KUVO_HOME:box.home,KUVO_TEST_RELAY:relay.url,KUVO_RELAY_URL:relay.url,BUN_BE_BUN:"1"},box.home,"pipe");
  }else{
    const relay=JSON.parse(fs.readFileSync(path.join(box.home,"run","agents","relay.json"),"utf8"));
    client=launch(binary.file,["mcp"],{...box.env,KUVO_HOME:box.home,KUVO_TOOL_RELAY:`http://127.0.0.1:${relay.port}/mcp`,KUVO_TOOL_RELAY_KEY:`Bearer ${relay.keys["@engine"]}`,KUVO_TOOL_TIMEOUT:"960000"},box.home,"pipe");
  }
  const lines=createInterface({input:client.child.stdout})[Symbol.asyncIterator]();let next=0;
  async function mcp(method,params={}){const id=++next;client.child.stdin.write(JSON.stringify({jsonrpc:"2.0",id,method,params})+"\n");const line=await lines.next();assert.ok(!line.done,client.log());const answer=JSON.parse(line.value);assert.equal(answer.id,id,line.value);assert.ok(answer.result,line.value);return answer.result;}
  await mcp("initialize",{protocolVersion:"2025-06-18"});
  return {client,call:(name,args={})=>mcp("tools/call",{name,arguments:args,_meta:{"ai.opencode/sessionID":"ses_browser"}}),close:async()=>{client.child.stdin.end();await client.exited;}};
}
function said(result){assert.notEqual(result.isError,true,JSON.stringify(result));return result.content.filter(c=>c.type==="text").map(c=>c.text).join("\n");}
function indexOf(snapshot,pattern){const line=snapshot.split("\n").find(line=>pattern.test(line)&&/^\[\d+\]/.test(line));assert.ok(line,`No ${pattern} in:\n${snapshot}`);return Number(/^\[(\d+)\]/.exec(line)[1]);}
async function webPage(t){
  const http=await import("node:http"),{once}=await import("node:events");
  const png=Buffer.from("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+7b4kAAAAASUVORK5CYII=","base64");
  const main=`<!doctype html><meta charset=utf-8><title>Browser fixture</title><link rel=icon href=/favicon.ico><h1>Local browser fixture</h1><button style="position:fixed;left:320px;top:10px;width:100px;height:40px;z-index:9" onclick="confirm('Fixture question')">Ask a question</button><button onclick="document.getElementById('clicked').textContent='Clicked locally'">Click me</button><p id=clicked>Waiting</p><input placeholder="Type here" oninput="document.getElementById('typed').textContent=this.value"><p id=typed>Nothing typed</p><label>Attach <input type=file name=attachment onchange="document.getElementById('attached').textContent=this.files[0].name"></label><p id=attached>Nothing attached</p><a href=/download>Download example</a><a href=/login>Sign-in fixture</a><div style="height:1800px"></div><p>Bottom of fixture</p>`;
  const login=`<!doctype html><title>Ask fixture</title><h1>Sign-in fixture</h1><label>Username <input autocomplete=username name=username></label><label>Password <input type=password autocomplete=current-password name=password></label><button onclick="document.getElementById('done').textContent='Sign-in entered'">Submit sign-in</button><p id=done></p><label>One-time code <input autocomplete=one-time-code name=code oninput="document.getElementById('codeDone').textContent='Code entered'"></label><p id=codeDone></p>`;
  const server=http.createServer((req,res)=>{if(req.url==="/download"){res.writeHead(200,{"content-type":"application/octet-stream","content-disposition":"attachment; filename=example.txt"});res.end("local download");}else if(req.url==="/favicon.ico"){res.writeHead(200,{"content-type":"image/png"});res.end(png);}else{res.writeHead(200,{"content-type":"text/html"});res.end(req.url==="/login"?login:main);}});
  server.listen(0,"127.0.0.1");await once(server,"listening");t.after(()=>new Promise(resolve=>server.close(resolve)));
  return `http://localhost:${server.address().port}`;
}

// The Chrome executable is a read-only fixture. The profile, downloads,
// temporary files and every user setting it sees live in the scratch home.
const chromeBuild=process.env.KUVO_CONFORMANCE_CHROME_BUILD;
test("real Chrome reads, acts, prints, uploads, downloads, changes tabs and streams its view",{timeout:180000,skip:!chromeBuild||process.platform==="win32"},async t=>{
  const {computer,relay}=await bench(t);const fs=await import("node:fs");const {write,until,boxBinary}=await import("./adapter.mjs");const {sessionOf}=await import("./engine.mjs");
  const url=await webPage(t);
  const box=await computer("Chrome",undefined,()=>({KUVO_BROWSER_IDLE:"15",KUVO_BROWSER_TAB_IDLE:"3600"}));
  const work=path.join(box.home,"work"),upload=path.join(work,"local.txt"),pdf=path.join(work,"page.pdf");
  write(upload,"local upload");box.engine.sessions.push(sessionOf({id:"ses_browser",title:"Browser",directory:work}));
  fs.mkdirSync(path.join(box.home,"browser"),{recursive:true});fs.symlinkSync(fs.realpathSync(chromeBuild),path.join(box.home,"browser","current"));
  const resident=box.start(),record=await resident.ready(),ask=box.connect(record);
  assert.equal((await ask("GET","kuvo/info")).status,200);
  const processes=async()=>{const cp=await import("node:child_process");const list=cp.execFileSync("/bin/ps",["-axww","-o","command="],{encoding:"utf8"});return list.split("\n").filter(line=>line.includes(`--user-data-dir=${path.join(box.home,"browser","profile")}`)).length;};
  assert.equal(await processes(),0,"opening the resident leaves Chrome stopped");
  const mcp=await browserMcp(box,record,relay);
  assert.equal(await processes(),0,"listing tools leaves Chrome stopped");
  let view,problem,browsing=0;
  try{
    let page=said(await mcp.call("browser_navigate",{url}));assert.match(page,/Page: Browser fixture/);assert.match(page,/Local browser fixture/);
    browsing=await processes();assert.ok(browsing>0);
    const text=said(await mcp.call("browser_text"));assert.match(text,/Bottom of fixture/);
    page=said(await mcp.call("browser_snapshot"));
    page=said(await mcp.call("browser_click",{index:indexOf(page,/Click me/)}));assert.match(page,/Clicked locally/);
    page=said(await mcp.call("browser_type",{index:indexOf(page,/Type here/),text:"Typed locally"}));assert.match(page,/Typed locally/);
    page=said(await mcp.call("browser_upload",{index:indexOf(page,/name="attachment"/),files:[upload]}));assert.match(page,/local.txt/);
    page=said(await mcp.call("browser_click",{index:indexOf(page,/Download example/)}));
    await until(()=>fs.existsSync(path.join(box.home,"browser","downloads","example.txt")),"The download wasn't kept",15000);
    assert.equal(fs.readFileSync(path.join(box.home,"browser","downloads","example.txt"),"utf8"),"local download");
    const picture=await mcp.call("browser_screenshot");const image=picture.content.find(c=>c.type==="image");assert.equal(image.mimeType,"image/png");assert.deepEqual(Buffer.from(image.data,"base64").subarray(0,8),Buffer.from("89504e470d0a1a0a","hex"));
    assert.match(said(await mcp.call("browser_save_pdf",{path:pdf})),/Saved the page as a/);assert.equal(fs.readFileSync(pdf).subarray(0,5).toString(),"%PDF-");
    page=said(await mcp.call("browser_scroll",{direction:"down",pages:2}));assert.match(page,/Scroll: [1-9]\d* px above/);
    assert.match(said(await mcp.call("browser_wait",{text:"Bottom of fixture"})),/is on the page/);
    page=said(await mcp.call("browser_new_tab",{url:url+"/login"}));assert.match(page,/Ask fixture/);
    let tabs=said(await mcp.call("browser_tabs"));assert.match(tabs,/Browser fixture/);assert.match(tabs,/Ask fixture.*\(in use\)/);
    const mainTab=Number(/\[(\d+)\].*Browser fixture/.exec(tabs)[1]);
    const loginTab=Number(/\[(\d+)\].*Ask fixture/.exec(tabs)[1]);
    assert.match(said(await mcp.call("browser_tabs",{action:"switch",tab:mainTab})),/Browser fixture/);
    page=said(await mcp.call("browser_tabs",{action:"switch",tab:loginTab}));assert.match(page,/Ask fixture/);
    // The real ask route receives synthetic credentials. Neither secret is
    // part of the model-facing reply or numbered snapshot.
    const username=indexOf(page,/name="username"/),password=indexOf(page,/name="password"/);
    box.engine.messages.set("ses_browser", [{id:"msg_browser",content:[{type:"tool",name:"browser_sign_in",callID:"call_browser_signin",state:{status:"running",input:{username,password}}}]}]);
    const signIn=mcp.call("browser_sign_in",{username,password});
    const signAsk=await Promise.race([signIn.then(result=>{throw new Error("Sign-in returned before an ask: "+JSON.stringify(result));}),until(async()=>{const result=await ask("GET","kuvo/asks");return result.json?.asks?.find(a=>a.kind==="signIn");},()=>{const file=path.join(box.home,"run","browser-asks.json");const asks=fs.existsSync(file)?JSON.parse(fs.readFileSync(file,"utf8")).asks.map(a=>a.kind):[];return "No sign-in ask; disk asks: "+JSON.stringify(asks)+"; MCP: "+mcp.client.log();},45000)]);
    assert.equal((await ask("POST",`kuvo/asks/${signAsk.id}`,{answer:"signIn",username:"fixture-user",password:"fixture-secret"})).status,200);
    page=said(await signIn);assert.match(page,/Filled .*for/);assert.doesNotMatch(page,/fixture-secret|fixture-user/);
    const code=mcp.call("browser_ask_code",{index:indexOf(page,/name="code"/),prompt:"Fixture code"});
    const codeAsk=await until(async()=>{const result=await ask("GET","kuvo/asks");return result.json?.asks?.find(a=>a.kind==="code");},"No code ask",20000);
    assert.equal((await ask("POST",`kuvo/asks/${codeAsk.id}`,{answer:"code",code:"123456"})).status,200);
    page=said(await code);assert.match(page,/Filled the code/);assert.doesNotMatch(page,/123456/);
    const closed=said(await mcp.call("browser_tabs",{action:"close",tab:loginTab}));
    assert.match(closed,/Closed tab/);
    const remaining=await until(async()=>{const tabs=said(await mcp.call("browser_tabs"));return !/Ask fixture/.test(tabs)&&tabs;},"Closed tab stayed in the list",10000);
    assert.match(remaining,/Browser fixture/);
    assert.equal((await ask("GET","kuvo/browser/view")).status,426);
    view=await openView(record);
    const pages=await view.until(m=>m.method==="Kuvo.pages"&&m.params.pages.length>0);
    const target=pages.params.active||pages.params.pages[0].targetId;
    view.send({id:1,method:"Kuvo.view",params:{targetId:target,width:480,height:320,maxFps:10}});
    const openedView=await view.until(m=>m.id===1);assert.ok(openedView.result,JSON.stringify(openedView));
    const frame=await view.until(m=>m.binary);
    const length=frame.binary.readUInt32BE(0),header=JSON.parse(frame.binary.subarray(4,4+length));
    assert.equal(header.method,"Kuvo.frame");assert.equal(header.params.targetId,target);assert.ok(header.params.width>0&&header.params.height>0);
    assert.deepEqual(frame.binary.subarray(4+length,7+length),Buffer.from("ffd8ff","hex"));
    view.send({method:"Kuvo.frameAck",params:{frameId:header.params.frameId}});
    // The local page has a fixed button that opens a synchronous dialog.
    // Its answer cannot wait behind the input call it holds.
    view.send({id:4,method:"Kuvo.input",params:{targetId:target,kind:"mouse",type:"mousePressed",x:370,y:30,button:"left",clickCount:1}});
    const press=await view.until(m=>m.id===4);assert.ok(press.result,JSON.stringify(press));
    view.send({id:5,method:"Kuvo.input",params:{targetId:target,kind:"mouse",type:"mouseReleased",x:370,y:30,button:"left",clickCount:1}});
    await view.until(m=>m.method==="Kuvo.dialog"&&m.params.targetId===target);
    view.send({id:6,method:"Kuvo.dialogAnswer",params:{targetId:target,accept:true}});
    const answer=await view.until(m=>m.id===6);assert.ok(answer.result,JSON.stringify(answer));
    const release=await view.until(m=>m.id===5);assert.ok(release.result,JSON.stringify(release));
    view.send({id:2,method:"Kuvo.go",params:{targetId:target,input:url}});
    const navigatedView=await view.until(m=>m.id===2);assert.ok(navigatedView.result,JSON.stringify(navigatedView));
    const nextFrame=await view.until(m=>m.binary && !m.binary.subarray(4+m.binary.readUInt32BE(0)).equals(frame.binary.subarray(4+length)));
    const nextLength=nextFrame.binary.readUInt32BE(0),nextHeader=JSON.parse(nextFrame.binary.subarray(4,4+nextLength));
    assert.ok(nextHeader.params.frameId!==header.params.frameId);
    assert.notDeepEqual(nextFrame.binary.subarray(4+nextLength),frame.binary.subarray(4+length));
    view.send({method:"Kuvo.frameAck",params:{frameId:nextHeader.params.frameId}});
    const history=await ask("GET","kuvo/browser/history");assert.equal(history.status,200);assert.ok(history.json.sites.some(s=>s.host===new URL(url).host));
  }catch(error){problem=error;throw error;}
  finally{
    await view?.close();await mcp.close();
    try {await until(async()=>!fs.existsSync(path.join(box.home,"run","browser.pid"))&&await processes()===0,()=>{let pid="none";try{pid=fs.readFileSync(path.join(box.home,"run","browser.pid"),"utf8");}catch{}return "The idle test Chrome didn't exit; keeper PID file: "+pid+"; socket still exists: "+fs.existsSync(path.join(box.home,"run","browser.sock"));},45000);}
    catch(error){ if(problem) console.error("Browser cleanup:",error.message); else throw error; }
    t.diagnostic(`Chrome processes with the fixture profile: cold 0, tool listing 0, browsing ${browsing}, after idle ${await processes()}`);
  }
});

// Tiny WebSocket client for the same authenticated loopback boundary as the app.
async function openView(record){
  const net=await import("node:net"),crypto=await import("node:crypto"),{once}=await import("node:events"),{until}=await import("./adapter.mjs");
  const socket=net.createConnection(record.port,"127.0.0.1");let bytes=Buffer.alloc(0),upgraded=false,problem;const messages=[];
  socket.on("data",chunk=>{
    bytes=Buffer.concat([bytes,chunk]);
    if(!upgraded){const end=bytes.indexOf("\r\n\r\n");if(end<0)return;const head=bytes.subarray(0,end).toString();if(!/^HTTP\/1\.1 101/.test(head)){problem=head;return;}bytes=bytes.subarray(end+4);upgraded=true;}
    for(;;){if(bytes.length<2)return;let length=bytes[1]&127,offset=2;if(length===126){if(bytes.length<4)return;length=bytes.readUInt16BE(2);offset=4;}else if(length===127){if(bytes.length<10)return;length=Number(bytes.readBigUInt64BE(2));offset=10;}if(bytes.length<offset+length)return;const payload=bytes.subarray(offset,offset+length),opcode=bytes[0]&15;bytes=bytes.subarray(offset+length);if(opcode===1){for(const line of payload.toString().split("\n").filter(Boolean))messages.push(JSON.parse(line));}else if(opcode===2)messages.push({binary:Buffer.from(payload)});}
  });
  socket.on("error",error=>problem=error.message);
  await once(socket,"connect");socket.write(`GET /kuvo/browser/view HTTP/1.1\r\nHost: localhost\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Version: 13\r\nSec-WebSocket-Key: ${crypto.randomBytes(16).toString("base64")}\r\nAuthorization: Basic ${Buffer.from("opencode:"+record.password).toString("base64")}\r\nX-Kuvo-Route-Version: 1\r\n\r\n`);
  await until(()=>{if(problem)throw new Error(problem);return upgraded;},"View socket did not open",15000);
  return {send(message){const payload=Buffer.from(JSON.stringify(message)+"\n");const mask=crypto.randomBytes(4),head=payload.length<126?Buffer.from([0x81,0x80|payload.length]):Buffer.from([0x81,0xfe,payload.length>>8,payload.length&255]);for(let i=0;i<payload.length;i++)payload[i]^=mask[i%4];socket.write(Buffer.concat([head,mask,payload]));},until:predicate=>until(()=>{if(problem)throw new Error(problem);const index=messages.findIndex(predicate);return index<0?null:messages.splice(index,1)[0];},"No matching browser view message",15000),close:async()=>{if(socket.destroyed)return;const closed=once(socket,"close");const mask=crypto.randomBytes(4);socket.write(Buffer.concat([Buffer.from([0x88,0x80]),mask]));socket.end();const timer=setTimeout(()=>socket.destroy(),1000);try{await closed;}finally{clearTimeout(timer);}}};
}

test("site data lists and removes only matching sites without starting Chrome", {timeout:60000}, async t=>{
  const {computer}=await bench(t);
  const fs=await import("node:fs"),{DatabaseSync}=await import("node:sqlite");
  const box=await computer("Site data");
  const profile=path.join(box.home,"browser","profile","Default"),network=path.join(profile,"Network");
  fs.mkdirSync(network,{recursive:true});
  const file=path.join(network,"Cookies"), db=new DatabaseSync(file);
  db.exec("CREATE TABLE cookies(host_key TEXT,name TEXT,path TEXT,expires_utc INTEGER,has_expires INTEGER,is_persistent INTEGER,value TEXT)");
  const expires=(Date.now()+86400000+11644473600000)*1000;
  const insert=db.prepare("INSERT INTO cookies VALUES(?,?, '/',?,1,1,'fixture')");
  for(const [host,name] of [[".example.test","fixture-login"],["api.example.test","fixture-setting"],["other.test","theme"]])insert.run(host,name,expires);
  insert.run("expired.test","expired",expires-172800000000);
  db.close();
  const indexed=path.join(profile,"IndexedDB");
  for(const name of ["https_api.example.test_0.indexeddb.leveldb","https_other.test_0.indexeddb.leveldb"])fs.mkdirSync(path.join(indexed,name),{recursive:true});
  const ask=box.connect(await box.start().ready());
  assert.deepEqual((await ask("GET","kuvo/browser/sites")).json,{sites:[{site:"example.test",kinds:["cookies","storage"]},{site:"other.test",kinds:["cookies","storage"]}]});
  assert.equal((await ask("DELETE","kuvo/browser/sites/test")).status,400);
  assert.deepEqual((await ask("DELETE","kuvo/browser/sites/example.test")).json,{sites:[{site:"other.test",kinds:["cookies","storage"]}]});
  assert.deepEqual(JSON.parse(fs.readFileSync(path.join(box.home,"browser","clear-sites.json"),"utf8")).sort(),["api.example.test","example.test"]);
  assert.ok(!fs.existsSync(path.join(indexed,"https_api.example.test_0.indexeddb.leveldb")));
  assert.ok(fs.existsSync(path.join(indexed,"https_other.test_0.indexeddb.leveldb")));
  assert.deepEqual((await ask("DELETE","kuvo/browser/sites")).json,{sites:[]});
  assert.ok(!fs.existsSync(indexed));
  assert.ok(!fs.existsSync(path.join(box.home,"run","browser.pid")));
});

test("a Mac opens only its registered browser app, once, and forwards the browser tool", {timeout:90000,skip:process.platform!=="darwin"}, async t=>{
  const {computer,relay}=await bench(t),fs=await import("node:fs");
  const {boxBinary,startBox,request,write,until}=await import("./adapter.mjs");
  const http=await import("node:http"),crypto=await import("node:crypto"),{once}=await import("node:events");
  const box=await computer("Mac browser"),binary=boxBinary();
  write(path.join(box.home,"box","kind"),"mac\n");
  const bundle=path.join(box.env.HOME,"Fixture Browser.app"), desktop=path.join(box.env.HOME,"Fixture Desktop.app");
  const password="fixture-helper-password",token="fixture-browser-tool-token";
  let starts=0, calls=0, record, tools;
  const app={"x-kuvo-from":"app"},mac={"x-kuvo-from":"mac"};
  const host=http.createServer(async(req,res)=>{
    let body="";for await(const chunk of req)body+=chunk;
    const value=JSON.parse(body||"{}");
    res.setHeader("content-type","application/json");
    if(req.url==="/hello"){
      const proof=crypto.createHmac("sha256",password).update(`kuvo-host-hello\0${value.nonce}\0${host.address().address}:${host.address().port}`).digest("hex");
      res.end(JSON.stringify({proof}));
    }else if(req.url==="/app/open" && req.headers.authorization==="Bearer "+password){
      starts++;
      assert.equal(value.bundle,bundle);
      const answer=await request(record,"POST","kuvo/box/app",{bundle,tools},app);
      assert.equal(answer.status,200,answer.text);
      res.end("{}");
    }else if(req.url==="/tools" && req.headers.authorization==="Bearer "+token){
      calls++;
      assert.equal(value.method,"tools/call");
      assert.equal(value.params.name,"browser_snapshot");
      assert.equal(req.headers["x-kuvo-session"],"ses_browser");
      res.end(JSON.stringify({jsonrpc:"2.0",id:value.id,result:{content:[{type:"text",text:"Fixture Mac browser"}],isError:false}}));
    }else{res.statusCode=403;res.end("{}");}
  });
  host.listen(0,"127.0.0.1");await once(host,"listening");
  t.after(()=>{host.close();host.closeAllConnections();});
  const url=`http://127.0.0.1:${host.address().port}`;
  tools={url:url+"/tools",token};
  const child=startBox({binary,home:box.home,relay,env:box.env,mac:true});
  t.after(()=>child.stop());record=await child.ready();
  // The resident publishes its port before wiring its routes.
  await until(async()=>{try{return (await request(record,"GET","kuvo/info")).status===200;}catch{return false;}},()=>`Mac resident did not answer: ${child.log()}`,15000,100);
  write(path.join(box.home,"run","host.json"),JSON.stringify({url,password}));
  const ask=async(method,route,body,headers)=>{
    try{return await request(record,method,route,body,headers);}
    catch(error){throw new Error(`${method} ${route}: ${error.message}; resident exit ${child.child.exitCode}/${child.child.signalCode}`,{cause:error});}
  };
  try{
  assert.equal((await ask("POST","kuvo/box/app",{desktop},app)).status,200);
  assert.equal((await ask("POST","kuvo/box/app/open",{},mac)).status,503);
  assert.equal(starts,0,"The desktop app is not the browser app");
  assert.equal((await ask("POST","kuvo/box/app",{bundle,tools},app)).status,200);
  assert.equal((await ask("POST","kuvo/box/app",{tools:null},app)).status,200);
  const opened=await Promise.all([ask("POST","kuvo/box/app/open",{},mac),ask("POST","kuvo/box/app/open",{},mac)]);
  assert.ok(opened.every(r=>r.status===200),JSON.stringify(opened.map(r=>r.json)));
  assert.equal(starts,1);
  assert.equal((await ask("POST","kuvo/box/app/open",{gone:"older-fixture-token"},mac)).status,200);
  assert.equal(starts,1,"A stale report cannot retire a newer registration");
  const mcp=await browserMcp(box,record,relay);
  try{assert.equal(said(await mcp.call("browser_snapshot")),"Fixture Mac browser");assert.equal(calls,1);}
  finally{await mcp.close();}
  assert.equal((await ask("GET","kuvo/browser/view")).status,404);
  assert.ok(!fs.existsSync(path.join(box.home,"run","browser.pid")));
  }catch(error){t.diagnostic(child.log());throw error;}
});

test("the shared capture clock backs off still images and keeps the input wake-up floor", async()=>{
  const {framePolicy}=await import("../../box/src/browser/frame-policy.js");
  const policy=framePolicy();
  assert.equal(policy.due(0),true);
  assert.equal(policy.captured("first",0,10),true);
  assert.equal(policy.due(99),false);
  assert.equal(policy.due(100),true);
  assert.equal(policy.captured("first",100,10),false);
  assert.equal(policy.due(349),false);
  assert.equal(policy.due(350),true);
  policy.dirty();
  assert.equal(policy.due(199),false);
  assert.equal(policy.due(200),true);
  assert.equal(policy.captured("changed",200,10),true);
});

test("native Windows reports that Chrome is not available instead of starting an installer", {timeout:30000,skip:process.platform!=="win32"}, async t=>{
  const {computer}=await bench(t),fs=await import("node:fs");
  const box=await computer("Native browser");
  const ask=box.connect(await box.start().ready());
  const status=await ask("GET","kuvo/browser");
  assert.equal(status.status,200);
  assert.equal(status.json.state,"failed");
  assert.match(status.json.message,/runs on Linux and macOS; this machine is win32/);
  assert.equal((await ask("POST","kuvo/browser",{now:true})).status,202);
  assert.ok(!fs.existsSync(path.join(box.home,"browser","profile")));
  assert.ok(!fs.existsSync(path.join(box.home,"run","browser.pid")));
});
