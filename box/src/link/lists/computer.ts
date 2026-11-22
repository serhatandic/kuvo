// The identity a setup list gives its local rows, also used to check an addressed start.

import { ownComputerName } from "../computer-name";
import { ownKey } from "../peers/known";
import { acceptedName } from "../setup/membership";
import { readLink } from "../state";

export function listComputer(link = readLink()) {
  const key = link && ownKey(link);
  const name = ownComputerName();
  return { id: key ?? "here", name: link ? acceptedName(link, key, name) : name };
}
