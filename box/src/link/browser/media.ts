import { additiveObjectFormat } from "../../shared/format-version";
import { isObject } from "../../shared/json";

const mediaFormat = additiveObjectFormat("Browser media header");

export function encodeBrowserFrame(meta: Record<string, unknown>, jpeg: Buffer): Buffer {
  const header = Buffer.from(JSON.stringify(mediaFormat.write({ method: "Kuvo.frame", params: meta })));
  const length = Buffer.alloc(4);
  length.writeUInt32BE(header.length);
  return Buffer.concat([length, header, jpeg]);
}

export function readBrowserFrame(data: Buffer) {
  if (data.length < 5) throw new Error("Incomplete browser media frame.");
  const length = data.readUInt32BE(0);
  if (length === 0 || length >= data.length - 4) throw new Error("Invalid browser media header length.");
  const header = mediaFormat.read(JSON.parse(data.subarray(4, 4 + length).toString("utf8")));
  if (header.method !== "Kuvo.frame" || !isObject(header.params) || !Number.isSafeInteger(header.params.frameId)) {
    throw new Error("Invalid browser media header.");
  }
  return header.params;
}
