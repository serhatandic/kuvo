// The caller gives this writer a detached tagged checkout and an isolated account.
import path from "node:path";
import { makeMachine, mailVector, save, settingsVectors, tags } from "./tagged.mjs";

const [tag, commit, script, destination] = process.argv.slice(2);
const code = await import(script);
const { home, env, setup, peer } = makeMachine({ script, code }, tag, destination);
const ssh = tag === tags[1] ? settingsVectors(script, home, env) : [];
save(path.join(destination, "vectors.json"), { tag, commit, setup, ssh, mail: mailVector(code, tag, peer) });
