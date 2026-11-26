// Antigravity through each resident, on a scripted `agy acp` (acp.test.ts, antigravity-agy.test.ts):
// its account and models from its home, signing in with Google's code, its modes, its reads
// through Kuvo, its plan's limit, and an account from before agy.
import assert from "node:assert/strict";
import fs from "node:fs";
import path from "node:path";
import { after, before, describe, test } from "node:test";
import { boxBinary, request, until, write } from "./adapter.mjs";
import { answerSessionAsk, askFirst, assertCommandCard, assertMatch, headers, idles, sessionAsks, texts, toolCalls } from "./claude.mjs";
import { providers, scriptedAcp } from "./acp.mjs";

const provider = providers.agy;
const oauth = `kuvo/agents/api/integration/${provider}/connect/oauth`;
const typescript = boxBinary().typescript;
const skip = typescript && process.platform === "win32" ? "The reference bridge runs on Unix." : false;
const words = (text) => ({ update: { sessionUpdate: "agent_message_chunk", content: { type: "text", text } } });

const offered = async (listener) => {
  const answer = await request(listener, "GET", "kuvo/models/offered", undefined, headers, 45000);
  assert.equal(answer.status, 200, answer.text);
  return answer.json.models.filter((model) => model.providerID === provider);
};
const account = async (listener) => {
  const answer = await request(listener, "GET", "kuvo/agents/api/integration", undefined, headers);
  assert.equal(answer.status, 200, answer.text);
  return answer.json.data.find((row) => row.id === provider);
};

/** A resident of its own for one case. */
async function withAgy(options, check, prepare = () => {}) {
  const acp = await scriptedAcp({ names: ["agy"], grok: false, ...options });
  try {
    prepare(acp);
    await acp.start();
    await check(acp);
  } catch (error) {
    throw new Error([error.message, acp.log()].join("\n"), { cause: error });
  } finally {
    await acp.close();
  }
}

describe("Antigravity's account", { skip }, () => {
  test("its account and models come from its home, without starting agy", { timeout: 60000 }, async () => {
    await withAgy({}, async (acp) => {
      const row = await account(acp.listener);
      assert.ok(row, "No Antigravity row.");
      const models = await offered(acp.listener);
      assert.deepEqual(models.map((model) => model.id).sort(), ["acp-claude", "acp-test"]);
      assert.ok(models.some((model) => model.name === "Claude Fixture (Antigravity)"), JSON.stringify(models.map((model) => model.name)));
      assert.equal(acp.started("agy").length, 0, "Listing started agy");
    });
  });

  test("signing in: Google's page, its code pasted back; then agy is asked to use it and list its models", { timeout: 90000 }, async () => {
    await withAgy({ agy: false }, async (acp) => {
      const started = await request(acp.listener, "POST", oauth, { methodID: "oauth-personal" }, headers, 20000);
      assert.equal(started.status, 200, started.text);
      const { attemptID, url } = started.json.data;
      assert.equal(url, "https://accounts.example.test/o/oauth2?client=agy");
      const pasted = await request(acp.listener, "POST", `${oauth}/${attemptID}/complete`, { code: "good-code" }, headers, 20000);
      assert.ok(pasted.status < 300, pasted.text);
      await until(async () => {
        const checked = await request(acp.listener, "GET", `${oauth}/${attemptID}`, undefined, headers);
        return checked.json?.data?.status === "complete";
      }, "The sign-in never finished.");
      assert.ok(acp.sent("authenticate").length >= 1, "agy wasn't asked to use its sign-in.");
      assert.ok((await offered(acp.listener)).some((model) => model.id === "acp-test"));
    });
  });

  test("signing in where agy isn't installed says so, with what to do", { timeout: 60000, skip: typescript && "The reference turns the reason into a generic line (Work Queue #992)." }, async () => {
    await withAgy({ names: [], agy: false }, async (acp) => {
      const started = await request(acp.listener, "POST", oauth, { methodID: "oauth-personal" }, headers, 20000);
      assert.ok(started.status >= 400, started.text);
      assert.match(started.json?.message ?? started.text, /install/i);
    });
  });

  /** The layout an account signed in before agy left: Google's earlier agent and its token (a stand-in). */
  const earlier = (token) => (acp) => {
    const root = path.join(acp.home, "agents", "antigravity");
    write(path.join(root, "1.3.0", ".complete"), "");
    write(path.join(root, "profile", "antigravity-acp", "acp_token.json"), JSON.stringify({ token: { refresh_token: token } }));
  };

  test("an account signed in before agy, with agy missing, says to install agy, without offering an update", { timeout: 60000, skip: typescript && "The reference still asks to update (Work Queue #992)." }, async () => {
    await withAgy({ names: [], agy: false }, async (acp) => {
      const said = (await account(acp.listener)).metadata.message;
      assert.doesNotMatch(said, /update/i);
      assert.match(said, /install agy/i);
    }, earlier("fixture-refresh"));
  });

  test("an account signed in before agy, with agy here, is signed in once agy takes that sign-in: no Google page", { timeout: 90000, skip: typescript && "The reference still asks to update (Work Queue #992)." }, async () => {
    await withAgy({ agy: false }, async (acp) => {
      const before = await account(acp.listener);
      assert.doesNotMatch(before.metadata.message, /update/i);
      assert.match(before.metadata.message, /sign in/i);
      assert.equal(acp.started("agy").length, 0, "Reading the account started agy");
      const started = await request(acp.listener, "POST", oauth, { methodID: "oauth-personal" }, headers, 20000);
      assert.equal(started.status, 200, started.text);
      const { attemptID } = started.json.data;
      await until(async () => {
        const checked = await request(acp.listener, "GET", `${oauth}/${attemptID}`, undefined, headers);
        return checked.json?.data?.status === "complete";
      }, "agy never took the earlier sign-in.");
      assert.equal(acp.heard().filter((line) => line.login).length, 0, "Google's page was asked for");
      assert.ok((await offered(acp.listener)).some((model) => model.id === "acp-test"));
    }, earlier("accepted-legacy"));
  });

  test("an account signed in before agy that agy won't take signs in on Google's page, as any", { timeout: 90000, skip: typescript && "The reference still asks to update (Work Queue #992)." }, async () => {
    await withAgy({ agy: false }, async (acp) => {
      const started = await request(acp.listener, "POST", oauth, { methodID: "oauth-personal" }, headers, 20000);
      assert.equal(started.status, 200, started.text);
      const { attemptID } = started.json.data;
      const page = await until(async () => {
        const checked = await request(acp.listener, "GET", `${oauth}/${attemptID}`, undefined, headers);
        return checked.json?.data?.url && checked.json.data;
      }, "Google's page never came.");
      assert.equal(page.url, "https://accounts.example.test/o/oauth2?client=agy");
    }, earlier("refused-legacy"));
  });
});

describe("Antigravity's sign-in as it runs", { skip }, () => {
  const id = "ses_agy_sign_in";
  const seeded = (acp) => acp.seed(id, provider, { directory: acp.home });
  const goesOn = async (acp, steps) => {
    acp.script("go on", [...steps, { wait: 1500 }, words("Still here.")]);
    await acp.prompt(id, "go on");
    const rows = await acp.idles(id);
    assert.ok(texts(rows).join("").includes("Still here."), JSON.stringify(idles(rows).at(-1)));
    assert.notEqual(idles(rows).at(-1).outcome, "failed", JSON.stringify(idles(rows).at(-1)));
    assert.equal(acp.started("agy").length, 1, "agy was started again");
    const row = await account(acp.listener);
    assert.equal(row.metadata.signInEnded, false, JSON.stringify(row.metadata));
  };

  test("a command it runs that opens a page (through $BROWSER, or writing $TMPDIR/url) doesn't sign you out: the turn goes on", { timeout: 60000 }, async () => {
    await withAgy({}, async (acp) => {
      await goesOn(acp, [{ opens: "https://accounts.google.com/o/oauth2/v2/auth?client_id=fixture" }]);
      assert.ok(acp.heard().some((line) => line.opened), "The command never ran.");
    }, seeded);
  });

  test("its sign-in words in what it shows (a command's output, a file it edits, its answer) aren't it asking: the turn goes on", { timeout: 60000 }, async () => {
    const said = 'const SIGN_IN_LINE: &str = "Open the following link to authenticate the ACP server:";';
    await withAgy({}, async (acp) => {
      await goesOn(acp, [
        { update: { sessionUpdate: "tool_call", toolCallId: "g1", kind: "search", title: "grep SIGN_IN_LINE", status: "pending" } },
        { update: { sessionUpdate: "tool_call_update", toolCallId: "g1", status: "completed", content: [{ type: "content", content: { type: "text", text: `48: ${said}` } }] } },
        { update: { sessionUpdate: "tool_call", toolCallId: "e1", kind: "edit", title: "edit_file", status: "completed", content: [{ type: "diff", path: path.join(acp.home, "antigravity.rs"), oldText: `${said}\nconst A: u8 = 1;`, newText: `${said}\nconst A: u8 = 2;` }] } },
        words(`It says "${said.slice(28, -2)}" when it wants you to sign in.`),
      ]);
    }, seeded);
  });

  test("agy's own words asking for a sign-in as it starts end the turn, saying to sign in again", { timeout: 60000 }, async () => {
    await withAgy({}, async (acp) => {
      acp.script("go on", [{ say: "Open the following link to authenticate the ACP server: https://accounts.google.com/o/oauth2/v2/auth?client_id=fixture" }, "hang"]);
      await acp.prompt(id, "go on");
      const rows = await acp.idles(id);
      const last = idles(rows).at(-1);
      assert.equal(last.outcome, "failed", JSON.stringify(last));
      assert.match(JSON.stringify(last), /sign in again/i);
      await until(async () => (await account(acp.listener)).metadata.signInEnded === true, "The account never said its sign-in ended.");
    }, seeded);
  });
});

describe("Antigravity's thinking levels", { skip }, () => {
  test("each model's levels, as agy lists them, are offered and kept with its models; the one picked is set once, as the conversation starts", { timeout: 90000 }, async () => {
    const id = "ses_agy_levels";
    await withAgy({}, async (acp) => {
      for (const turn of ["first", "second"]) {
        acp.script(turn, [words(`The ${turn}.`)]);
        await acp.prompt(id, turn);
        await acp.idles(id, turn === "first" ? 1 : 2);
      }
      const levels = acp.sent("session/set_config_option").filter((message) => message.params.configId === "thought_level");
      assert.deepEqual(levels.map((message) => message.params.value), ["xhigh"], JSON.stringify(acp.sent("session/set_config_option")));
      const models = await offered(acp.listener);
      assert.deepEqual(models.find((model) => model.id === "acp-test")?.efforts, ["low", "high", "xhigh"], JSON.stringify(models));
      assert.deepEqual(models.find((model) => model.id === "acp-fast")?.efforts ?? [], [], JSON.stringify(models));
      const kept = JSON.parse(fs.readFileSync(path.join(acp.home, "agents", "antigravity", "profile", "kuvo-models.json"), "utf8"));
      assert.deepEqual(kept.models.find((model) => model.value === "acp-test").efforts, ["low", "high", "xhigh"]);
      assert.equal("efforts" in kept.models.find((model) => model.value === "acp-fast"), false, "An empty list was written");
    }, (acp) => acp.seed(id, provider, { directory: acp.home, variant: "xhigh" }));
  });
});

describe("Antigravity's turns", { skip }, () => {
  let acp;
  const pool = { plain: [], ask: [] };
  before(async () => {
    if (skip) return;
    acp = await scriptedAcp({ names: ["agy"], grok: false });
    for (const [kind, permissions] of [["plain"], ["ask", askFirst]]) {
      for (let index = 1; index <= 6; index++) {
        const id = `ses_agy_${kind}_${index}`;
        const directory = path.join(acp.home, "projects", id);
        write(path.join(directory, "notes.txt"), "line one\nline two\n");
        acp.seed(id, provider, { directory, permissions });
        pool[kind].push({ id, directory });
      }
    }
    await acp.start();
  });
  after(async () => acp?.close());

  async function begin(steps, { ask = false } = {}) {
    const session = pool[ask ? "ask" : "plain"].shift();
    assert.ok(session, "Seed more sessions.");
    acp.script(`go ${session.id}`, steps);
    await acp.prompt(session.id, `go ${session.id}`);
    return session;
  }

  test("Approve All puts it in its yolo mode, and its words show", { timeout: 60000 }, async () => {
    const { id } = await begin([words("Hello from Antigravity.")]);
    const rows = await acp.idles(id);
    assert.ok(texts(rows).join("").includes("Hello from Antigravity."));
    assert.ok(acp.sent("session/set_config_option").some((message) => message.params.configId === "mode" && message.params.value === "yolo"), JSON.stringify(acp.sent("session/set_config_option")));
  });

  test("the app's card: its command is the conversation's ask, and Allow Once given there reaches it", { timeout: 60000 }, async () => {
    const { id } = await begin([
      { update: { sessionUpdate: "tool_call", toolCallId: "t7", kind: "execute", title: "Run `make`", status: "pending", rawInput: { command: "make" } } },
      { ask: ["session/request_permission", { toolCall: { toolCallId: "t7" }, options: [{ optionId: "yes", kind: "allow_once", name: "Allow" }, { optionId: "always", kind: "allow_always", name: "Always" }, { optionId: "no", kind: "reject_once", name: "No" }] }] },
      words("Done."),
    ], { ask: true });
    const card = await until(async () => (await sessionAsks(acp.listener, id)).find((ask) => ask.kind === "permission"), "No card for the command.");
    assertCommandCard(card, "make");
    await answerSessionAsk(acp.listener, id, card.id, { answer: "once" });
    assert.deepEqual((await acp.answered("session/request_permission")).result, { outcome: { outcome: "selected", optionId: "yes" } });
    await acp.idles(id);
    assert.deepEqual(await sessionAsks(acp.listener, id), []);
  });

  test("a file it has Kuvo read comes back to it", { timeout: 60000 }, async () => {
    const notes = path.join(pool.plain[0].directory, "notes.txt");
    const { id } = await begin([
      { update: { sessionUpdate: "tool_call", toolCallId: "r1", kind: "read", title: "client_view_file", status: "pending", locations: [{ path: notes }] } },
      { ask: ["fs/read_text_file", { path: notes, line: 2, limit: 1 }] },
      words("Read it."),
    ]);
    const answer = await acp.answered("fs/read_text_file");
    assert.deepEqual(answer.result, { content: "line two" });
    const rows = await acp.idles(id);
    assertMatch(toolCalls(rows).find((tool) => (tool.tool ?? tool.name) === "read"), { state: { status: "completed" } });
  });

  test("its whole answer saying its plan's limit is reached ends the turn on that model's limit", { timeout: 60000 }, async () => {
    const { id } = await begin([words("Usage Limit Reached. You have reached your current quota for this period. Your limit will reset in 6 days, 19 hours.")]);
    const rows = await acp.idles(id);
    assertMatch(idles(rows).at(-1), { outcome: "failed" });
    const reason = idles(rows).at(-1).error ?? idles(rows).at(-1).reason;
    assertMatch(reason, { type: "limit", limit: { scope: "model" } });
  });

  test("a session made only to check the sign-in leaves nothing in its home", { timeout: 20000 }, async () => {
    const brain = path.join(acp.home, "agents", "antigravity", "profile", "antigravity-acp", "brain");
    assert.equal(fs.existsSync(brain) ? fs.readdirSync(brain).length : 0, 0);
  });
});
