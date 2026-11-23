// Node's directory entry for `node --test protocol/conformance`.
const area = process.env.KUVO_CONFORMANCE_AREA;
if (!area || area === "resident") await import("./resident.test.mjs");
if (!area || area === "two-boxes") await import("./two-boxes.test.mjs");
if (!area || area === "engine-sessions") await import("./engine-sessions.test.mjs");
if (!["resident", "two-boxes", "engine-sessions"].includes(area)) await import("./routes.test.mjs");
