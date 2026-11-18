import "./safety";
import { expect, test } from "bun:test";
import { matchBoxRoute } from "../src/link/server/router";
import { BOX_ROUTES } from "../src/shared/routes/box";
import { SKILL_ROUTES } from "../src/shared/routes/skills";

test.each([
  ["POST", "kuvo/phones/remove", "phones"],
  ["GET", "kuvo/skill-trash", "skills.trash"],
  ["POST", "kuvo/skill-trash/:id/restore", "skills.trash"],
] as const)("%s %s declares the feature used by its dispatcher", (method, route, feature) => {
  const path = route.replace(":id", "example");
  const contract = [...BOX_ROUTES, ...SKILL_ROUTES].find(shape => shape.route === `${method} ${route}`);
  const dispatch = matchBoxRoute(path, { method, path, headers: {} });
  expect({ contract: contract?.feature, dispatcher: dispatch?.entry.feature }).toEqual({ contract: feature, dispatcher: feature });
});
