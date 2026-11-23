// Node's directory entry for `node --test protocol/conformance`.
const area = process.env.KUVO_CONFORMANCE_AREA;
if (!area || area === "resident") await import("./resident.test.mjs");
if (!area || area === "peers") await import("./peers.test.mjs");
if (!area || area === "two-boxes") await import("./two-boxes.test.mjs");
if (!area || area === "holder") await import("./holder.test.mjs");
if (!area || area === "compat") await import("./compat.test.mjs");
if (!area || area === "clock") await import("./clock.test.mjs");
if (!area || area === "engine-sessions") await import("./engine-sessions.test.mjs");
if (!area || area === "engine-typed") await import("./engine-typed.test.mjs");
if (!area || area === "engine-delivery") await import("./engine-delivery.test.mjs");
if (!area || area === "engine-asks") await import("./engine-asks.test.mjs");
if (!area || area === "engine-search") await import("./engine-search.test.mjs");
if (!area || area === "engine-models") await import("./engine-models.test.mjs");
if (!area || area === "engine-live") await import("./engine-live.test.mjs");
if (area === "engine-golden") await import("./engine-golden.test.mjs");
if (!area || area === "engine-worktree") await import("./engine-worktree.test.mjs");
if (!area || area === "stored-formats") await import("./stored-formats.test.mjs");
if (![
  "resident", "peers", "two-boxes", "holder", "compat", "clock", "stored-formats",
  "engine-sessions", "engine-typed", "engine-delivery", "engine-asks", "engine-search",
  "engine-models", "engine-live", "engine-golden", "engine-worktree",
].includes(area)) await import("./routes.test.mjs");
