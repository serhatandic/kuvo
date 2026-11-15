import "./safety";
import { expect, test } from "bun:test";
import { fitsShape } from "../src/shared/shape";
import { SETTINGS_CHANGE, SETTINGS_PAGE_ROUTES } from "../src/shared/routes/settings-pages";

test("PATCH Settings declares and checks its change body", () => {
  const shape = SETTINGS_PAGE_ROUTES.find(route => route.route === "PATCH kuvo/settings/:section")!.body!;
  expect(shape).toEqual(SETTINGS_CHANGE);
  const change = { row: "older-budget", value: 0, was: "read" };
  for (const body of [change, { ...change, confirmed: true }, { ...change, confirmed: false }]) expect(fitsShape(shape, body)).toBe(true);
  for (const body of [{ ...change, confirmed: "true" }, { ...change, extra: true }, { row: "older-budget", was: "read" }, { ...change, was: 1 }]) expect(fitsShape(shape, body)).toBe(false);
});
