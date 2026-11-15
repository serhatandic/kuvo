#!/bin/sh
# Writes protocol/vectors/routes/box: what a box's release brings that its routes read (the
# integrations it offers, the popular MCP servers), from the box's own table
# (box/src/shared/integrations), as a release writes them. The machine's runner of the route
# scenarios (box/bridge-tests/contract.test.js) sets its box up with them, and the box's tests
# hold them to the table (box/test/release-catalogs.test.ts). Run it after changing the table,
# then the runners.
#   scripts/route-vectors.sh            integrations.json and mcp-catalog.json
#   scripts/route-vectors.sh --mac-kit  mac-bot-kit.json too: what's this Mac's own that the retired
#                                       Swift Mac app gave its own box to make bots with, from that
#                                       app's own code (KuvoCodeTests/RouteContractTests, through
#                                       scripts/test-app.sh: it builds and runs that app's tests)
#   BUN   the Bun to run with (as scripts/build-box.sh finds it)
# (approval.json there is the box's own, written by its tests: left as it is.)
set -eu
cd "$(dirname "$0")/.."

. scripts/real-app-guard.sh
if [ -z "${BUN:-}" ]; then
  if command -v bun >/dev/null 2>&1; then
    BUN=bun
  else
    for engine in "$HOME/Library/Application Support/run.kuvo.code."*/engine/*/opencode; do
      [ -x "$engine" ] && BUN=$engine
    done
  fi
fi
[ -n "${BUN:-}" ] || { echo "scripts/route-vectors.sh needs Bun: install it, or name one with BUN=." >&2; exit 1; }
real_app_guard "scripts/route-vectors.sh" "$BUN"

mkdir -p protocol/vectors/routes/box
BUN_BE_BUN=1 "$BUN" -e '
import fs from "node:fs";
import { integrationsCatalog, mcpCatalog } from "./box/src/shared/integrations/catalogs.ts";
const write = (name, catalog) => fs.writeFileSync(`protocol/vectors/routes/box/${name}`, JSON.stringify(JSON.parse(catalog), null, 2) + "\n");
write("integrations.json", integrationsCatalog(false));
write("mcp-catalog.json", mcpCatalog());
'

if [ "${1:-}" = "--mac-kit" ]; then
    TEMP="${TMPDIR:-/tmp}"
    OUT="${TEMP%/}/kuvo-route-vectors"
    rm -rf "$OUT"
    # Passed to the test as KUVO_WRITE_ROUTE_VECTORS (xcodebuild hands TEST_RUNNER_ variables on).
    TEST_RUNNER_KUVO_WRITE_ROUTE_VECTORS="$OUT" ONLY="KuvoCodeTests/RouteContractTests/theMachinesRunnerHasWhatAMacLeavesThere()" ./scripts/test-app.sh >/dev/null
    [ -s "$OUT/mac-bot-kit.json" ] || { echo "The test didn't write mac-bot-kit.json." >&2; exit 1; }
    mv -f "$OUT/mac-bot-kit.json" protocol/vectors/routes/box/
    rm -rf "$OUT"
fi
echo "Wrote protocol/vectors/routes/box"
