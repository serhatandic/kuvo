// Node's directory entry for `node --test protocol/conformance`.
const areas = new Map([
  ["resident", "./resident.test.mjs"],
  ["peers", "./peers.test.mjs"],
  ["removal", "./removal.test.mjs"],
  ["adding", "./adding.test.mjs"],
  ["two-boxes", "./two-boxes.test.mjs"],
  ["moves", "./moves.test.mjs"],
  ["holder", "./holder.test.mjs"],
  ["compat", "./compat.test.mjs"],
  ["clock", "./clock.test.mjs"],
  ["engine-sessions", "./engine-sessions.test.mjs"],
  ["engine-live", "./engine-live.test.mjs"],
  ["engine-worktree", "./engine-worktree.test.mjs"],
  ["stored-formats", "./stored-formats.test.mjs"],
]);
const area = process.env.KUVO_CONFORMANCE_AREA;
if (!area) {
  for (const file of areas.values()) await import(file);
}
if (!areas.has(area)) await import("./routes.test.mjs");
else await import(areas.get(area));
