// The computer's display name, shared by account and setup words and Computers.
import { plainName } from "../shared/text";
import { IS_MAC } from "./home";
import { computerName } from "./mac/link";
import { namedBoxes, ownKey } from "./peers/known";
import { macLink, readLink } from "./state";

export function ownComputerName(): string {
  let link;
  try { link = readLink(); } catch {}
  const key = link ? ownKey(link) : undefined;
  const named = !IS_MAC && link && key ? namedBoxes(link).find(box => box.key === key)?.name : undefined;
  return plainName((IS_MAC ? macLink.config?.name : named) || computerName());
}

export function computerInSentence(request: { fromApp?: boolean }): string {
  return request.fromApp === true && IS_MAC ? "this Mac" : ownComputerName();
}
