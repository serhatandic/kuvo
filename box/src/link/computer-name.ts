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
  // A Mac by the name its app gave it; a machine by the name its Macs' records give it (renamed
  // there), else its own link's name. The system's name only when the box was given none.
  const named = !IS_MAC && link && key ? namedBoxes(link).find(box => box.key === key)?.name : undefined;
  return plainName(IS_MAC ? macLink.config?.name : plainName(named) || link?.name) || computerName();
}

export function computerInSentence(request: { fromApp?: boolean }): string {
  return request.fromApp === true && IS_MAC ? "this Mac" : ownComputerName();
}
