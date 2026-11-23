// Node's directory entry for `node --test protocol/conformance`.
const area = process.env.KUVO_CONFORMANCE_AREA;
if (!area || area === "resident") await import("./resident.test.mjs");
if (!area || area === "peers") await import("./peers.test.mjs");
if (!area || area === "removal") await import("./removal.test.mjs");
if (!area || area === "setup-judge") await import("./setup-judge.test.mjs");
if (!area || area === "adding") await import("./adding.test.mjs");
if (!area || area === "two-boxes") await import("./two-boxes.test.mjs");
if (!area || area === "holder") await import("./holder.test.mjs");
if (!area || area === "compat") await import("./compat.test.mjs");
if (!area || area === "clock") await import("./clock.test.mjs");
if (!area || area === "engine-sessions") await import("./engine-sessions.test.mjs");
if (!area || area === "engine-live") await import("./engine-live.test.mjs");
if (!area || area === "engine-worktree") await import("./engine-worktree.test.mjs");
if (!area || area === "stored-formats") await import("./stored-formats.test.mjs");
if (!["setup-judge", "removal", "adding", "resident", "peers", "two-boxes", "holder", "compat", "clock", "stored-formats", "engine-sessions", "engine-live", "engine-worktree"].includes(area)) await import("./routes.test.mjs");
