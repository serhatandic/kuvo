// Node's directory entry for `node --test protocol/conformance`.
const area = process.env.KUVO_CONFORMANCE_AREA;
if (!area || area === "resident") await import("./resident.test.mjs");
if (!area || area === "peers") await import("./peers.test.mjs");
if (!area || area === "two-boxes") await import("./two-boxes.test.mjs");
if (!area || area === "holder") await import("./holder.test.mjs");
if (!area || area === "compat") await import("./compat.test.mjs");
if (!area || area === "stored-formats") await import("./stored-formats.test.mjs");
if (!["resident", "peers", "two-boxes", "holder", "compat", "stored-formats"].includes(area)) await import("./routes.test.mjs");
