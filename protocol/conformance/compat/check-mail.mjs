// Check both noise directions through the wire implementation exported by the TS bundle.
import assert from "node:assert/strict";
import { pathToFileURL } from "node:url";
import { readJSON } from "../adapter.mjs";

const [binary, vectors] = process.argv.slice(2);
const { frame, readFrame, Frame, sealOneWay, openOneWay, publicOf, MAIL_PROLOGUE } = await import(pathToFileURL(binary).href);
const mail = readJSON(vectors).mail;
const hex = key => Buffer.from(mail[key], "hex");
const [sender, recipient, ephemeral, packet] = ["sender", "recipient", "ephemeral", "frame"].map(hex);
const payload = Buffer.from(mail.plaintext, "utf8");
assert.deepEqual(openOneWay(readFrame(packet).body, recipient, publicOf(sender)).payload, payload);
const sealed = sealOneWay(payload, sender, publicOf(recipient), MAIL_PROLOGUE, ephemeral).message;
assert.deepEqual(frame(Frame.mail, 0, sealed), packet);
