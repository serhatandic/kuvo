import { LABEL_RESTORE_PREVIEW_FEATURE, LABEL_RESTORE_ROUTE } from "../../shared/routes/work-queue";
import { peersOf } from "../peers/known";
import { followedSetup } from "../setup/record";
import { readLink } from "../state";
import { forwardAct, takesForwarded, type Acted } from "./forward";
import { restoreLabels } from "./labels";

export async function previewLabelRestore(request: { library: unknown; force: boolean }, signal?: AbortSignal): Promise<Acted> {
  const link = readLink();
  const setup = followedSetup(link);
  if (setup?.role !== "member") return previewLabelsHere(request.library, request.force);
  const main = peersOf(link).find(peer => peer.key === setup.primary);
  if (!main) return { status: 503, value: { message: "This box can't reach the main computer." } };
  if (!takesForwarded(main.key, LABEL_RESTORE_PREVIEW_FEATURE)) {
    return { status: 409, value: { message: "Update the box on the main computer before previewing a restore from another computer." } };
  }
  const reply = await forwardAct(main, "The label library", { area: "ownership", method: "GET", item: LABEL_RESTORE_ROUTE, body: request },
    signal, link, Date.now(), { memory: true });
  return "refused" in reply ? { status: reply.status, value: { message: reply.refused } } : reply;
}

export function previewLabelsHere(library: unknown, force: boolean): Acted {
  const outcome = restoreLabels(library, true, force);
  return "message" in outcome ? { status: outcome.status, value: { message: outcome.message } } : { status: 200, value: outcome.result };
}
