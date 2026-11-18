import "./safety";
import { expect, test } from "bun:test";
import { versionedRoutes } from "../src/link/server/format";

test("a versioned request normalizes header names and refuses conflicts before entering the handler", async () => {
  const entered: unknown[] = [];
  const handle = versionedRoutes(async function* ({ headers }) {
    entered.push(headers);
    yield { head: { status: 200, headers: {} } };
  });
  const request = { method: "POST", path: "kuvo/info" };
  for (const headers of [
    { "X-Kuvo-Route-Version": "2" },
    { "x-kuvo-route-version": "1", "X-Kuvo-Route-Version": "2" },
    { Accept: "application/json", accept: "text/plain" },
  ]) {
    expect((await handle({ ...request, headers }, {}).next()).value).toMatchObject({ head: { status: 400 } });
  }
  expect(entered).toEqual([]);
  const headers = { "X-Kuvo-Route-Version": "1", Accept: "application/json", accept: "application/json" };
  expect((await handle({ ...request, headers }, {}).next()).value).toMatchObject({ head: { status: 200 } });
  expect(entered).toEqual([{ "x-kuvo-route-version": "1", accept: "application/json" }]);
});
