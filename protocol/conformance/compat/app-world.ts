// The engine fixture is an OpenCode 2.0.32 database. This reuses the external
// engine and Keychain stand-ins from the whole staged-move scenario.
import { mac07 } from "../../../box/test/rehome-run-world";

const [root] = process.argv.slice(2);
console.log(JSON.stringify(mac07(root, () => {}, { at: "mac", engine: null })));
