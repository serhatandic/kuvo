// Node's directory entry for `node --test protocol/conformance`.
const areas = new Map([
  ["resident", "./resident.test.mjs"],
  ["peers", "./peers.test.mjs"],
  ["removal", "./removal.test.mjs"],
  ["setup-judge", "./setup-judge.test.mjs"],
  ["ssh-adding", "./ssh-adding.test.mjs"],
  ["adding", "./adding.test.mjs"],
  ["two-boxes", "./two-boxes.test.mjs"],
  ["moves", "./moves.test.mjs"],
  ["rehome", "./rehome.test.mjs"],
  ["holder", "./holder.test.mjs"],
  ["agents", "./holder.test.mjs"],
  ["compat", "./compat.test.mjs"],
  ["clock", "./clock.test.mjs"],
  ["engine-sessions", "./engine-sessions.test.mjs"],
  ["engine-typed", "./engine-typed.test.mjs"],
  ["engine-delivery", "./engine-delivery.test.mjs"],
  ["engine-asks", "./engine-asks.test.mjs"],
  ["engine-search", "./engine-search.test.mjs"],
  ["engine-models", "./engine-models.test.mjs"],
  ["engine-live", "./engine-live.test.mjs"],
  ["engine-worktree", "./engine-worktree.test.mjs"],
  ["backups", "./backups.test.mjs"],
  ["stored-formats", "./stored-formats.test.mjs"],
  ["files", "./files.test.mjs"],
  ["git", "./git.test.mjs"],
  ["updates", "./updates.test.mjs"],
  ["vault", "./vault.test.mjs"],
]);
const area = process.env.KUVO_CONFORMANCE_AREA;
if (!area) {
  for (const file of areas.values()) await import(file);
}
if (area === "engine-golden") await import("./engine-golden.test.mjs");
else if (!areas.has(area)) await import("./routes.test.mjs");
else await import(areas.get(area));
