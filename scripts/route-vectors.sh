#!/bin/sh
# Writes box/test/fixtures/routes/box: what a box's release brings that its routes read (the
# integrations it offers, the popular MCP servers), from the box's own table
# (box/src/shared/integrations), as a release writes them. The machine's runner of the route
# scenarios (box/bridge-tests/contract.test.js) sets its box up with them, and the box's tests
# hold them to the table (box/test/release-catalogs.test.ts). Run it after changing the table,
# then the runners.
#   scripts/route-vectors.sh            integrations.json and mcp-catalog.json
# (mac-bot-kit.json is a captured 0.7 fixture.)
#   BUN   the Bun to run with (as scripts/build-box.sh finds it)
# (approval.json there is the box's own, written by its tests: left as it is.)
set -eu
cd "$(dirname "$0")/.."
[ "$#" -eq 0 ] || { echo "Usage: scripts/route-vectors.sh" >&2; exit 2; }

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

mkdir -p box/test/fixtures/routes/box
BUN_BE_BUN=1 "$BUN" -e '
import fs from "node:fs";
import { integrationsCatalog, mcpCatalog } from "./box/src/shared/integrations/catalogs.ts";
const write = (name, catalog) => fs.writeFileSync(`box/test/fixtures/routes/box/${name}`, JSON.stringify(JSON.parse(catalog), null, 2) + "\n");
write("integrations.json", integrationsCatalog(false));
write("mcp-catalog.json", mcpCatalog());
'

echo "Wrote box/test/fixtures/routes/box"
