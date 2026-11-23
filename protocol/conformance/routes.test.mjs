// The shared route scenarios through a real resident's HTTP interface.
import assert from "node:assert/strict";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { test } from "node:test";
import { boxBinary, isolatedEnv, localRelay, readJSON, request, scratch, startBox, until, write } from "./adapter.mjs";
import { standInEngine } from "./engine.mjs";
import { routeFixtures, seed } from "./fixtures.mjs";
import { filled, mismatch, offShape, pathIn, unnamed } from "./match.mjs";
import { referenceGaps } from "./reference-gaps.mjs";

const vectors = new URL("../../box/test/fixtures/routes/", import.meta.url);
const area = process.env.KUVO_CONFORMANCE_AREA;
const macFiles = new Set(["artifacts.json", "bots.json", "features.json"]);
const files = [...fs.readdirSync(vectors).filter(name => name.endsWith(".json")), "features.json"].sort();
const binary = boxBinary();
const isOurs = (only, kind) => only === undefined || only === kind;

const selected = files.filter(file => !area || file === area + ".json");
if (area && !selected.length) throw new Error(`Unknown route area: ${area}`);

test("routes", { concurrency: 3 }, async t => {
  const runs = [];
  for (const file of selected) {
    const source = file === "features.json" ? new URL("../vectors/routes/features.json", import.meta.url) : new URL(file, vectors);
    for (const scenario of readJSON(source).scenarios) {
      for (const kind of ["machine", ...(macFiles.has(file) ? ["mac"] : [])]) {
        const name = `${file.slice(0, -5)} [${kind}] ${scenario.name}`;
        const skip = !isOurs(scenario.only, kind) ? scenario.why : binary.typescript && referenceGaps.get(name);
        runs.push(t.test(name, { skip, timeout: 60000 }, t => run(t, scenario, kind)));
      }
    }
  }
  await Promise.all(runs);
});

async function run(t, scenario, kind) {
  const root = fs.realpathSync(scratch());
  const home = path.join(root, "box");
  const env = isolatedEnv(root);
  const mac = kind === "mac";
  const engine = await standInEngine(root, env);
  const relay = await localRelay();
  const children = [];
  t.after(async () => {
    for (const child of children.reverse()) await child.stop();
    await Promise.all([engine.close(), relay.close()]);
    fs.rmSync(root, { recursive: true, force: true, maxRetries: 3 });
  });
  seed(home, root, mac, engine.record);
  // The source scenarios started with a machine's folder before enrollment.
  // Reserve it from auto-setup except when a scenario brings peer records.
  if (!mac && !scenario.steps.some(step => step.given?.peers)) {
    write(path.join(root, ".kuvo-code-adding"), "");
  }
  const { fixtures, vars } = routeFixtures({ binary, home, root, env, engine, children });
  // The old runner injected this name. The resident announces the computer's actual hostname.
  vars.computer = os.hostname();
  let record;
  for (const [index, step] of scenario.steps.entries()) {
    if (!isOurs(step.only, kind)) continue;
    const where = `step ${index + 1}${step.send ? ` (${step.send})` : ""}`;
    if (step.given) {
      for (const [key, value] of Object.entries(step.given)) {
        assert.ok(fixtures[key], `${where}: unknown fixture ${key}`);
        await fixtures[key](filled(value, vars), vars);
      }
    } else if (step.send) {
      if (!record) {
        const box = startBox({ binary, home, env: { ...env, KUVO_SANDBOX: "off" }, relay, mac });
        children.push(box);
        record = await box.ready();
      }
      const [method, rawRoute] = step.send.split(" ");
      const route = filled(rawRoute, vars);
      const expectation = step.expect[kind] ?? step.expect;
      const expect = filled(expectation, vars);
      if (expect.json?.name === "lab.invalid" && route === "kuvo/info") expect.json.name = vars.computer;
      for (const row of [...(Array.isArray(expect.json?.asks) ? expect.json.asks : []), ...(Array.isArray(expect.json?.bots) ? expect.json.bots : []), expect.json?.bot]) {
        if (row?.machine === "lab.invalid") row.machine = vars.computer;
      }
      // HTTP rejects a path separator inside one segment before dispatch.
      if (["kuvo/skills/..%2Fescape", "kuvo/skills/%E0%A4%A"].includes(route)) {
        assert.equal((await request(record, method, route, filled(step.body, vars))).status, 403, where);
        continue;
      }
      const ask = () => request(record, method, route, filled(step.body, vars));
      let answer = await ask();
      // These reads show projections updated by the engine stream and filesystem watches.
      // Wait for the expected projection once it can answer, without replaying mutations.
      if (method === "GET" && ["kuvo/named", "kuvo/groups/joinable", "kuvo/worktrees"].includes(route.split("?")[0])
        && answer.status === expect.status && mismatch(expect.json, answer.json)) {
        await until(async () => {
          answer = await ask();
          return answer.status !== expect.status || mismatch(expect.json, answer.json) === null;
        }, () => `${where}: ${mismatch(expect.json, answer.json)}`, 25000, 100);
      }
      assert.equal(answer.status, expect.status, `${where}: ${answer.text.slice(0, 400)}`);
      let shapeProblem = offShape(method, route, answer);
      if (binary.typescript && shapeProblem?.includes("serverGeneration isn't a field") && route.split("?")[0] === "kuvo/info") {
        // TS reference green: the resident adds these fields to its in-process reply shape.
        const { serverGeneration, serverBuild, ...json } = answer.json;
        assert.match(serverGeneration, /^[0-9a-f-]{36}$/);
        assert.equal(typeof serverBuild, "string");
        shapeProblem = offShape(method, route, { ...answer, json });
      }
      assert.equal(shapeProblem, null, where);
      if (expect.type) assert.ok(answer.type.startsWith(expect.type), `${where}: content-type ${answer.type}`);
      if (expect.text !== undefined) assert.equal(answer.text, expect.text, where);
      if (method.toUpperCase() === "HEAD") {
        // HTTP suppresses the body even when the in-process handler returned JSON.
        assert.equal(answer.text, "", where);
      } else if (expect.json !== undefined) {
        const why = mismatch(expect.json, answer.json) ?? (expect.whole ? unnamed(expect.json, answer.json) : null);
        assert.equal(why, null, where);
      }
      for (const [name, dotted] of Object.entries(step.save ?? {})) {
        const value = pathIn(answer.json, dotted);
        assert.notEqual(value, undefined, `${where}: nothing at ${dotted} to save as ${name}`);
        vars[name] = value;
      }
    } else if (step.engineGot) {
      const wanted = filled(step.engineGot, vars);
      const got = engine.asked.findLast(each => each.method === wanted.method && each.path === wanted.path);
      assert.ok(got, `${where}: engine received no ${wanted.method} ${wanted.path}`);
      assert.equal(mismatch(wanted.json ?? {}, got.json, "sent"), null, where);
    } else {
      assert.fail(`${where}: expected given, send, or engineGot`);
    }
  }
}
