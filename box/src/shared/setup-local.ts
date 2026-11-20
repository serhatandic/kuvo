// The agents bridge and updater use the authenticated holder socket to save setup choices
// and check the outcome of previous saves.
import fs from "node:fs";
import net from "node:net";
import path from "node:path";
import type { SignedDoc } from "./documents";
import type { SharedKind } from "./data-declarations";
import { isObject } from "./json";
import { INTERRUPTED_SAVE, PENDING_SAVE, type Receipt, type SetupSaveReceipt } from "./routes/setup-save";

export type { SetupSaveReceipt };

export const LOCAL_SETUP_SAVE = "kuvo/setup/save-local";

export interface LocalSetupValue {
  id: string;
  epoch: number;
  primary: boolean;
  sequence: number;
  value: unknown;
}

export interface SetupSaveReference {
  setup: string;
  operation: string;
  proof: SignedDoc;
}

export function setupSaveReference(value: unknown): SetupSaveReference | null {
  if (!isObject(value) || typeof value.setup !== "string" || typeof value.operation !== "string"
    || !isObject(value.proof) || typeof value.proof.d !== "string" || typeof value.proof.s !== "string") return null;
  return { setup: value.setup, operation: value.operation, proof: { d: value.proof.d, s: value.proof.s } };
}

export function setupSaveReceipt(value: unknown): SetupSaveReceipt | null {
  const reference = setupSaveReference(value);
  if (!reference || !isObject(value) || !Number.isSafeInteger(value.epoch)) return null;
  const { epoch, status, message } = value;
  if (status !== "pending" && status !== "committed" && status !== "interrupted") return null;
  const identity = { ...reference, epoch: Number(epoch) };
  if (status === "pending") return { ...identity, status, pending: true, message: typeof message === "string" ? message : PENDING_SAVE };
  if (status === "interrupted") return { ...identity, status, message: typeof message === "string" ? message : INTERRUPTED_SAVE };
  return { ...identity, status, ...(typeof message === "string" ? { message } : {}) };
}

const setups = new Map<string, { inode: number; mtime: number; link: Record<string, unknown> }>();

export function localSetupValue(run: string, kind: string): LocalSetupValue | null {
  try {
    const file = path.join(run, "link.json");
    const stat = fs.statSync(file);
    let kept = setups.get(run);
    if (kept?.inode !== stat.ino || kept.mtime !== stat.mtimeMs) {
      const link = JSON.parse(fs.readFileSync(file, "utf8"));
      if (!isObject(link)) return null;
      kept = { inode: stat.ino, mtime: stat.mtimeMs, link };
      setups.set(run, kept);
    }
    const link = kept.link;
    const setup = link?.setup;
    if (!isObject(setup) || setup.aside || (!setup.standby && !setup.sharedSaves && !(setup.checkpointed && kind === "setupSecurityAndReleaseChoice"))
      || typeof setup.id !== "string" || typeof setup.epoch !== "number") return null;
    const values = link.sharedSetup === setup.id && isObject(link.shared) ? link.shared : null;
    const entry = values && Object.hasOwn(values, kind) && isObject(values[kind]) ? values[kind] : null;
    return { id: setup.id, epoch: setup.epoch, primary: setup.role === "primary",
      sequence: typeof entry?.sequence === "number" ? entry.sequence : 0, value: entry?.value };
  } catch { return null; }
}

export type LocalSaveAnswer = { status: number; message?: string } & (
  | { receipt: Receipt; legacyPending?: never }
  | { receipt?: never; legacyPending?: { pending: true; operation: string } }
);
type LocalStatus = LocalSaveAnswer & { outcome?: Receipt["status"] };

export function askSetupOutcome(run: string, reference: SetupSaveReference, kind?: SharedKind): Promise<LocalSaveAnswer> {
  const { setup, operation, proof } = reference;
  return askLocalSetup(run, { id: setup, operation, proof, kind }, { setup, operation, proof });
}

export function askSetupSave(run: string, body: { id: string; epoch: number; kind: SharedKind; sequence: number; value: object }): Promise<LocalSaveAnswer> {
  return askLocalSetup(run, body, { setup: body.id, epoch: body.epoch });
}

// The holder also supports inspecting a known operation by id on the same setup, without a proof.
export function askSetupStatus(run: string, body: { id: string; operation: string }): Promise<LocalStatus> {
  return askLocalSetup(run, body);
}

function askLocalSetup(run: string, body: object, reference?: Partial<Receipt>): Promise<LocalStatus> {
  const fallback = { status: 503, message: "The box that keeps your setup isn't answering." };
  const socketPath = path.join(run, "vault.sock");
  let key: string;
  try {
    const stat = fs.lstatSync(socketPath);
    if (!stat.isSocket() || (stat.mode & 0o077) || (process.getuid && stat.uid !== process.getuid())) return Promise.resolve(fallback);
    key = fs.readFileSync(path.join(run, "socket.key"), "utf8").trim();
    if (!/^[0-9a-f]{64}$/.test(key)) return Promise.resolve(fallback);
  } catch { return Promise.resolve(fallback); }
  return new Promise(resolve => {
    const socket = net.createConnection(socketPath);
    const finish = (answer: LocalStatus = fallback) => { socket.destroy(); resolve(answer); };
    socket.setEncoding("utf8").setTimeout(8_000);
    socket.on("connect", () => socket.write(JSON.stringify({ hello: "", key }) + "\n"));
    socket.on("timeout", () => finish());
    socket.on("error", () => finish());
    socket.on("close", () => finish());
    let answer = "", greeted = false;
    socket.on("data", chunk => {
      answer += chunk;
      if (answer.length > 65_536) return finish();
      let newline = answer.indexOf("\n");
      if (newline < 0) return;
      if (!greeted) {
        greeted = true;
        answer = answer.slice(newline + 1);
        socket.write(JSON.stringify({ path: LOCAL_SETUP_SAVE, method: "POST", headers: {},
          body: Buffer.from(JSON.stringify(body)).toString("base64") }) + "\n");
        newline = answer.indexOf("\n");
        if (newline < 0) return;
      }
      try {
        const reply = JSON.parse(answer.slice(0, newline));
        const value: unknown = JSON.parse(Buffer.from(reply.body, "base64").toString());
        if (!Number.isInteger(reply.status)) return finish();
        const result = isObject(value) ? value : {};
        const outcome: unknown = result.status;
        const receipt = setupSaveReceipt({ ...reference, ...result });
        const response: Pick<LocalStatus, "status" | "message" | "outcome"> = { status: reply.status, message: typeof result.message === "string" ? result.message : undefined,
          ...(outcome === "pending" || outcome === "committed" || outcome === "interrupted" ? { outcome } : {}),
        };
        if (receipt) return finish({ ...response, receipt });
        if (reply.status === 202 && typeof result.operation === "string") {
          return finish({ ...response, legacyPending: { pending: true, operation: result.operation } });
        }
        finish(response);
      } catch { finish(); }
    });
  });
}
