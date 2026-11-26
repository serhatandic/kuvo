// A reproducible offline Git seed and the normal worktree creation/removal sequence.
// Usage: node protocol/conformance/differential/worktrees.mjs FOLDER
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { isolatedEnv, privateHome, write } from "../adapter.mjs";
import { git } from "../engine.mjs";

const folder=process.argv[2];
assert.ok(folder, "Give a scratch output folder");
const seed=path.join(path.resolve(folder),"seed");
const project=path.join(seed,"repo");
privateHome(seed);
write(path.join(seed,"folder-version.json"),'{"version":23}\n');
write(path.join(seed,"box","opencode.json"),'{}\n');
write(path.join(project,"README.md"),"First\n");
const env={...isolatedEnv(path.resolve(folder)),GIT_AUTHOR_DATE:"2026-01-01T00:00:00Z",GIT_COMMITTER_DATE:"2026-01-01T00:00:00Z"};
write(path.join(seed,"box","kind"),"machine");
git(env,project,"init","-q","-b","main");
git(env,project,"add","-A");
git(env,project,"commit","-qm","First");
write(path.join(folder,"requests.json"), JSON.stringify([
  {method:"PUT",path:"kuvo/projects?directory={{home}}/repo",body:{name:"Fixture"}},
  {method:"POST",path:"kuvo/worktrees",body:{project:"{{home}}/repo"}},
  {method:"GET",path:"kuvo/worktrees?records=1"},
  {method:"POST",path:"kuvo/worktrees/remove",body:{path:"{{home}}/worktrees/brave-otter-1",forget:true}},
  {method:"GET",path:"kuvo/worktrees?records=1"},
  {method:"GET",path:"kuvo/projects"},
],null,2));
