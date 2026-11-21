import { expect, test } from "bun:test";
import { pathToFileURL } from "node:url";
import { mapAttachmentPaths, visitAttachmentPaths } from "../src/shared/attachment-paths";
import { ATTACHED_HEADINGS, UNSENT_SUFFIX } from "../src/shared/message-block";

const from = "/source/attachments/ses_example/proof ü.bin";
const to = "/destination/attachments/ses_example/proof ü.bin";
const uri = pathToFileURL(from).href;
const target = pathToFileURL(to).href;
const mappings = new Map([[from, to]]);
const quote = `I wrote ${from} yesterday and saved "${uri}".`;
const block = `<attached-files>\n${ATTACHED_HEADINGS[1]}\n${from}\n</attached-files>`;
const phone = "<from-phone>\nThe user sent this from Kuvo Code on their iPhone.\n</from-phone>";
const text = `${quote}\n\n${block}\n \n${phone}\n \t`;
const relocated = text.replace(block, block.replace(from, to));

test("engine and native attachment fields relocate while quoted evidence and command history remain exact", () => {
  const user = {
    type: "user",
    text,
    metadata: { kuvo: { parts: { words: quote, paths: [from], comments: [{ text: quote }] } } },
    files: [{ source: { type: "uri", uri: from } }, { uri: uri + "?pick=1#page=2", name: quote }],
  };
  const tool = { type: "tool", state: { input: { files: [{ uri }] }, output: quote,
    content: [{ type: "text", text: block }, { type: "file", uri }] } };
  const assistant = { type: "assistant", text, content: [{ type: "text", text: block }, tool] };
  const mappedAssistant = { ...assistant, content: [assistant.content[0], { ...tool, state: {
    ...tool.state, content: [tool.state.content[0], { type: "file", uri: target }],
  } }] };
  const shell = { type: "shell", command: `cat "${from}"`, output: { output: from } };
  const engine = { info: { role: "user" }, parts: [{ type: "text", text }, { type: "file", url: uri }] };
  const bare = { type: "user", text: from };
  const exports = { info: { title: from }, messages: [user, assistant, shell, engine, bare] };
  const result = mapAttachmentPaths(exports, mappings, "transcript");
  const expected = {
    ...exports,
    messages: [{
      ...user, text: relocated,
      metadata: { kuvo: { parts: { words: quote, paths: [to], comments: [{ text: quote }] } } },
      files: [{ source: { type: "uri", uri: to } }, { uri: target + "?pick=1#page=2", name: quote }],
    }, mappedAssistant, shell, { ...engine, parts: [{ type: "text", text: relocated }, { type: "file", url: target }] }, bare],
  };
  expect(result).toEqual(expected);
});

test("queued prose, escaped blocks and unsent legacy paths stay as written", () => {
  const stayed = block.replace(from, from + UNSENT_SUFFIX);
  const escaped = block.replace("<attached-files>", "<attached-files> ");
  const waiting = [
    { shown: { text: quote }, message: { text, files: [{ uri }] } },
    { shown: { text: stayed }, message: { text: stayed } },
    { shown: { text: escaped }, message: { text: escaped } },
    { shown: { text: block }, message: { text: block + "\n\n" + phone, metadata: { kuvo: { parts: { words: block } } } } },
  ];
  const queues = { ses_example: { paused: true, waiting } };
  const mapped = mapAttachmentPaths(queues, mappings, "queues");
  expect(mapped.ses_example.waiting).toEqual([
    { shown: { text: quote }, message: { text: relocated, files: [{ uri: target }] } },
    waiting[1], waiting[2], waiting[3],
  ]);
  const seen: { path: string; stayed: boolean }[] = [];
  visitAttachmentPaths(queues, "queues", file => { seen.push(file); return undefined; });
  expect(seen).toEqual([{ path: from, stayed: false }, { path: from, stayed: false }, { path: from, stayed: true }]);
  expect(mapAttachmentPaths({ type: "text", text }, mappings, "part").text).toBe(text);
});
