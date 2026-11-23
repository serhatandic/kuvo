// Scenario and shape matching from the TypeScript bridge contract runner.
import fs from "node:fs";

// `{{name}}` in a string, from the scenario's own (`save`, `as`) and the box's places;
// `{{now+3600}}` is the time then, in seconds. A string that's only one of them takes its value.
export function filled(value, vars) {
  if (typeof value === "string") {
    const whole = value.match(/^\{\{([^{}]+)\}\}$/);
    if (whole) return lookup(whole[1], vars);
    return value.replace(/\{\{([^{}]+)\}\}/g, (_, name) => String(lookup(name, vars)));
  }
  if (Array.isArray(value)) return value.map(each => filled(each, vars));
  if (value && typeof value === "object") return Object.fromEntries(Object.entries(value).map(([key, inner]) => [key, filled(inner, vars)]));
  return value;
}
function lookup(name, vars) {
  const time = name.match(/^now([+-]\d+)?$/);
  if (time) return Math.floor(Date.now() / 1000) + Number(time[1] ?? 0);
  if (!(name in vars)) throw new Error(`{{${name}}} isn't set`);
  return vars[name];
}

const kinds = { "<string>": "string", "<number>": "number", "<bool>": "boolean" };
// Why `actual` isn't what `expected` says, or null when it is (CONTRIBUTING.md, "Route scenarios").
export function mismatch(expected, actual, at = "body") {
  if (typeof expected === "string") {
    if (expected === "<any>") return actual === undefined ? `${at} is missing` : null;
    if (expected === "<absent>") return actual === undefined ? null : `${at} should be left out, but is ${JSON.stringify(actual)}`;
    if (kinds[expected]) return typeof actual === kinds[expected] ? null : `${at} should be a ${kinds[expected]}, but is ${JSON.stringify(actual)}`;
    if (expected === "<object>") return actual && typeof actual === "object" && !Array.isArray(actual) ? null : `${at} should be an object, but is ${JSON.stringify(actual)}`;
    if (expected === "<array>") return Array.isArray(actual) ? null : `${at} should be a list, but is ${JSON.stringify(actual)}`;
    return actual === expected ? null : `${at} should be ${JSON.stringify(expected)}, but is ${JSON.stringify(actual)}`;
  }
  // Null and left out are the same to a client (its optionals).
  if (expected === null) return actual === null || actual === undefined ? null : `${at} should be null, but is ${JSON.stringify(actual)}`;
  if (typeof expected !== "object") return actual === expected ? null : `${at} should be ${JSON.stringify(expected)}, but is ${JSON.stringify(actual)}`;
  if (Array.isArray(expected)) {
    if (!Array.isArray(actual)) return `${at} should be a list, but is ${JSON.stringify(actual)}`;
    if (actual.length !== expected.length) return `${at} should have ${expected.length}, but has ${actual.length}: ${JSON.stringify(actual)}`;
    for (const [index, each] of expected.entries()) {
      const why = mismatch(each, actual[index], `${at}[${index}]`);
      if (why) return why;
    }
    return null;
  }
  if ("<includes>" in expected || "<excludes>" in expected || "<length>" in expected) {
    if (!Array.isArray(actual)) return `${at} should be a list, but is ${JSON.stringify(actual)}`;
    if ("<length>" in expected && actual.length !== expected["<length>"]) return `${at} should have ${expected["<length>"]}, but has ${actual.length}`;
    for (const each of expected["<includes>"] ?? []) if (!actual.some(item => !mismatch(each, item))) return `${at} has none like ${JSON.stringify(each)}: ${JSON.stringify(actual)}`;
    for (const each of expected["<excludes>"] ?? []) if (actual.some(item => !mismatch(each, item))) return `${at} has one like ${JSON.stringify(each)}: ${JSON.stringify(actual)}`;
    return null;
  }
  if ("<contains>" in expected) return typeof actual === "string" && actual.includes(expected["<contains>"]) ? null : `${at} should contain ${JSON.stringify(expected["<contains>"])}, but is ${JSON.stringify(actual)}`;
  if (!actual || typeof actual !== "object" || Array.isArray(actual)) return `${at} should be an object, but is ${JSON.stringify(actual)}`;
  for (const [key, each] of Object.entries(expected)) {
    const why = mismatch(each, actual[key], `${at}.${key}`);
    if (why) return why;
  }
  return null;
}

// A value in an answer, by its path (`bot.id`, `worktrees.0.path`).
export const pathIn = (value, dotted) => dotted.split(".").reduce((inner, key) => inner?.[key], value);

// ---- What each route answers (protocol/vectors/routes/shapes/answers.json): every answer, whole ----

const shapes = JSON.parse(fs.readFileSync(new URL("../vectors/routes/shapes/answers.json", import.meta.url)));
const show = value => (value === undefined ? "missing" : JSON.stringify(value)?.slice(0, 200));
const isPlain = value => value !== null && typeof value === "object" && !Array.isArray(value);
// Why `value` isn't of `shape`, at its first difference, or null when it is.
function unshaped(shape, value, at = "body") {
  if (value === undefined) return `${at} is missing`;
  if (shape === "any") return null;
  if (shape === "string" || shape === "number") return typeof value === shape ? null : `${at} should be a ${shape}, but is ${show(value)}`;
  if (shape === "bool") return typeof value === "boolean" ? null : `${at} should be true or false, but is ${show(value)}`;
  if (shape === "null") return value === null ? null : `${at} should be null, but is ${show(value)}`;
  if (!isPlain(shape)) return `${at}: ${show(shape)} isn't a shape`;
  if ("literal" in shape) return value === shape.literal ? null : `${at} should be ${show(shape.literal)}, but is ${show(value)}`;
  if ("list" in shape) {
    if (!Array.isArray(value)) return `${at} should be a list, but is ${show(value)}`;
    for (const [index, each] of value.entries()) {
      const why = unshaped(shape.list, each, `${at}[${index}]`);
      if (why) return why;
    }
    return null;
  }
  if ("map" in shape) {
    if (!isPlain(value)) return `${at} should be an object, but is ${show(value)}`;
    for (const [key, each] of Object.entries(value)) {
      const why = unshaped(shape.map, each, `${at}.${key}`);
      if (why) return why;
    }
    return null;
  }
  if ("object" in shape) {
    if (!isPlain(value)) return `${at} should be an object, but is ${show(value)}`;
    const fields = new Map(Object.entries(shape.object).map(([name, each]) => [name.replace(/\?$/, ""), { shape: each, optional: name.endsWith("?") }]));
    for (const key of Object.keys(value)) if (!fields.has(key)) return `${at}.${key} isn't a field it has`;
    for (const [name, field] of fields) {
      if (!(name in value)) {
        if (field.optional) continue;
        return `${at}.${name} is missing`;
      }
      const why = unshaped(field.shape, value[name], `${at}.${name}`);
      if (why) return why;
    }
    return null;
  }
  if ("oneOf" in shape) {
    const whys = shape.oneOf.map(each => unshaped(each, value, at));
    return whys.includes(null) ? null : `${at} is none of what it can be: ${whys.join("; ")}`;
  }
  return `${at}: ${show(shape)} isn't a shape`;
}

const routeOf = (pattern, method, route) => {
  const [wantedMethod, wantedPath] = pattern.split(" ");
  const wanted = wantedPath.split("/"), parts = route.split("/");
  return wantedMethod === method && wanted.length === parts.length && wanted.every((part, i) => part.startsWith(":") ? parts[i] !== "" : part === parts[i]);
};
// Why an answer isn't what its route answers (null when it is): every answer of every route, the
// box's own, not only what a scenario names.
export function offShape(method, requestPath, answer) {
  const route = requestPath.split("?")[0];
  const isJSON = answer.type.startsWith("application/json");
  const verb = method.toUpperCase();
  // A HEAD is held to its GET's shape, or has no body.
  const known = shapes.routes.find(each => routeOf(each.route, verb, route)) ?? (verb === "HEAD" ? shapes.routes.find(each => routeOf(each.route, "GET", route)) : undefined);
  // (What's passed on is theirs, but for a route of the box's own among it: kuvo/agents/start.)
  if (!known && shapes.passedOn.some(prefix => route.startsWith(prefix))) return null;
  // A refusal says why in the one way all do, or in its route's own where it has one. A HEAD's
  // answer has no body.
  if (answer.status >= 400) return isJSON ? unshaped(known?.answers[answer.status] ?? shapes.error, answer.json, "the refusal") : null;
  if (verb === "HEAD" && answer.text === "") return known ? null : `HEAD ${route} has no shape in box/src/shared/routes`;
  if (!known) return `${verb} ${route} has no shape in box/src/shared/routes`;
  const shape = known.answers[answer.status];
  if (shape === undefined) return `${known.route} has no shape for ${answer.status}`;
  if (shape === "bytes") return null;
  return isJSON ? unshaped(shape, answer.json, "the answer") : `${known.route} answered ${answer.type || "no type"}, not JSON`;
}

// The first field an answer has that `expected` doesn't name, for a step that names it whole
// (`"whole": true`), so both boxes are held to the very same answer; null when there's none.
export function unnamed(expected, actual, at = "body") {
  if (Array.isArray(expected) && Array.isArray(actual)) {
    for (const [index, each] of expected.entries()) {
      const why = unnamed(each, actual[index], `${at}[${index}]`);
      if (why) return why;
    }
    return null;
  }
  const isObject = value => value !== null && typeof value === "object" && !Array.isArray(value);
  if (!isObject(expected) || !isObject(actual) || ["<includes>", "<excludes>", "<length>", "<contains>"].some(key => key in expected)) return null;
  for (const key of Object.keys(actual)) {
    if (!(key in expected)) return `${at}.${key} is there, but the whole answer doesn't name it`;
    const why = unnamed(expected[key], actual[key], `${at}.${key}`);
    if (why) return why;
  }
  return null;
}
