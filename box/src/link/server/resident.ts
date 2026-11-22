import crypto from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { exclusiveWhile } from "../../shared/files";
import { K, RUN } from "../home";
import { BRIDGE_VERSION } from "../version";
import { routeRefusalHeaders } from "./format";
import type { LinkHandler } from "./context";

export const RESIDENT_RELEASE = "// Kuvo resident serve v1\n";

/** The start and rollback target select their own client-process layout. */
export function residentInstalled(): boolean {
  return installedMarker(RESIDENT_RELEASE);
}

export function residentWorkersInstalled(): boolean {
  return installedMarker("// Kuvo resident workers v1\n");
}

function installedMarker(marker: string): boolean {
  try {
    const file = fs.openSync(path.join(K, "bin", "kuvo-link.js"), "r");
    try {
      const head = Buffer.alloc(512);
      return head.subarray(0, fs.readSync(file, head)).toString("utf8").includes(marker);
    } finally { fs.closeSync(file); }
  } catch { return false; }
}

export async function ownClientServer(work: (owner: ClientOwner) => Promise<void>): Promise<void> {
  await exclusiveWhile(path.join(RUN, ".client-owner"), async () => {
    if (clientServerStopped()) return;
    const owner = new ClientOwner();
    await work(owner);
    // The Mac startup returns once everything is listening. The process still owns the lock.
    await new Promise<void>(() => {});
  }, { untilExit: true, busy: "The resident box already runs." });
}

export const clientServerStopped = () => ["off", "paused"].some(mark => fs.existsSync(path.join(RUN, mark)));

export class ClientOwner {
  readonly generation = crypto.randomUUID();
  private active = 0;
  private leaving = false;

  info() {
    return { serverGeneration: this.generation, serverBuild: BRIDGE_VERSION };
  }

  entry(handle: LinkHandler): LinkHandler {
    const owner = this;
    return async function* (request, phone, signal) {
      if (owner.leaving) {
        yield { head: { status: 503, headers: routeRefusalHeaders } };
        yield { body: Buffer.from('{"message":"Kuvo is restarting."}') };
        return;
      }
      owner.active++;
      try { yield* handle(request, phone, signal); }
      finally { owner.active--; }
    };
  }

  async drain(): Promise<void> {
    this.leaving = true;
    const deadline = performance.now() + 1500;
    while (this.active && performance.now() < deadline) await new Promise(resolve => setTimeout(resolve, 25));
  }
}
