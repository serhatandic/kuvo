// The local byte stream used by SSH forwarding and the agent holder. Also embedded as source in
// the standalone holder so the listener and both clients select the same transport.
import * as ipcNet from "node:net";
import { once as ipcOnce } from "node:events";
import { createHmac as ipcHmac } from "node:crypto";

export function localIpc(unixPath, { platform = process.platform, windows, adapter } = {}) {
  if (platform === "win32") {
    if (!adapter || !windows) throw new Error("Kuvo's secure Windows named pipes are unavailable.");
    const { sid, homeKey, channel } = windows;
    if (!/^S-1-[0-9]+(?:-[0-9]+)+$/.test(sid) || !/^[0-9a-f]{64}$/.test(homeKey) ||
        !/^(ssh|holder:[0-9a-f]{12})$/.test(channel)) throw new Error("Invalid Windows local IPC identity.");
    const suffix = ipcHmac("sha256", Buffer.from(homeKey, "hex")).update("kuvo-local-ipc-v1\0" + sid + "\0" + channel).digest("hex");
    const policy = {
      path: `\\\\.\\pipe\\kuvo-${suffix}`,
      sid,
      sddl: `O:${sid}D:P(A;;GA;;;${sid})`,
      rejectRemote: true,
      firstInstance: true,
    };
    return {
      path: policy.path,
      connect: () => adapter.connect(policy),
      listen: (receive, options) => adapter.listen(policy, receive, options),
    };
  }
  return {
    path: unixPath,
    connect: () => ipcNet.createConnection(unixPath),
    listen(receive, { modeMask, onError, onListening } = {}) {
      const server = ipcNet.createServer(receive);
      if (onError) server.on("error", onError);
      if (onListening) server.once("listening", onListening);
      const ready = ipcOnce(server, "listening").then(() => {});
      const old = modeMask === undefined ? undefined : process.umask(modeMask);
      try { server.listen(unixPath); }
      finally { if (old !== undefined) process.umask(old); }
      return { ready, close: () => server.close() };
    },
  };
}
