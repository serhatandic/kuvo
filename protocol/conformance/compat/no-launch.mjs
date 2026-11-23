// The historical setup command detached a relay listener after writing its reply.
// Stand in for that launcher so the generator owns every process it runs.
import assert from "node:assert/strict";
import childProcess from "node:child_process";
import { realpathSync } from "node:fs";
import { syncBuiltinESMExports } from "node:module";
childProcess.spawn = (command, args, options) => {
  assert.equal(command, process.execPath);
  assert.deepEqual(args, [realpathSync(process.argv[1]), "run"]);
  assert.equal(options.detached, true);
  return { unref() {} };
};
syncBuiltinESMExports();
