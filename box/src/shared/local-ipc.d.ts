import type { Socket } from "node:net";

interface Listener {
  ready: Promise<void>;
  close(): void;
}

type Receive = (socket: Socket) => void;
type Callbacks = { onError?: (error: Error) => void; onListening?: () => void };

export interface WindowsPipePolicy {
  path: string;
  sid: string;
  sddl: string;
  rejectRemote: true;
  firstInstance: true;
}

/** The Windows OS boundary. listen applies the explicit DACL at creation, with local-only access
 * and a first-instance claim. connect checks the actual server's SID on the open pipe handle
 * before exposing the stream or sending application credentials. */
export interface WindowsPipeAdapter {
  connect(policy: WindowsPipePolicy): Promise<Socket>;
  listen(policy: WindowsPipePolicy, receive: Receive, callbacks?: Callbacks): Listener;
}

export function localIpc(unixPath: string, options?: {
  platform?: NodeJS.Platform;
  windows?: { sid: string; homeKey: string; channel: "ssh" | `holder:${string}` };
  adapter?: WindowsPipeAdapter;
}): {
  path: string;
  connect(): Socket | Promise<Socket>;
  listen(receive: Receive, options?: Callbacks & { modeMask?: number }): Listener;
};
