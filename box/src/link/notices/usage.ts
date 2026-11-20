import { askServices, readServices } from "../../shared/box-services";
import { followEvents } from "../../shared/event-stream";
import { RUN } from "../home";
import { limitsFollowers } from "../limits-events";
// The reader’s quota facts, delivered through the same native/push path as a run's news.
import { currentNotice, noticeAccountName, takeNotices } from "../../usage/notices";
import { keepTold } from "./told";
import type { NoticeHold } from "./hold";
import type { Notice } from "./notice";

import { usageDelivered } from "./usage-delivery";
const wording = {
  underTenPercent: ["Almost Out", "Under 10% usage remaining for this window."],
  healthyToClose: ["Cutting It Close", "Projected to finish close to your limit."],
  closeToRunningOut: ["Will Run Out", "Projected to finish before the limit resets."],
};

/** The selected holder alone pushes; other own computers keep the identical ID for their Mac's delivery. */
export function usageNotices(data: Record<string, unknown>, tell: (notice: Notice) => unknown): void {
  for (const fact of takeNotices(data.notices)) {
    if (!currentNotice(fact, Date.now())) continue;
    const [title, body] = wording[fact.milestone];
    const notice: Notice = { category: "usage", title, subtitle: `${fact.accountName ?? noticeAccountName(fact.service)} ${fact.windowName ?? "Limit"}`, body, thread: `usage:${fact.row}`, data: { notice: fact.id, row: fact.row, at: String(fact.at), ...(fact.resetsAt !== undefined ? { expires: String(fact.resetsAt) } : {}) } };
    // Keep stable native delivery facts first, independently of whether a phone is reachable.
    keepTold(notice, { at: fact.at });
    // Queue acceptance is not successful phone delivery. The hold journals only after sending;
    // a minute's catch-up retries an unconsumed fact while its short replay window is current.
    if (data.push === true && !usageDelivered(notice)) tell(notice);
  }
}

/** Both event and reconnect snapshots reconcile queued copies before any can be retried. */
export function usageSnapshot(data: Record<string, unknown>, tell: (notice: Notice) => unknown, hold?: Pick<NoticeHold, "reconcileUsage"> | null): void {
  const all = takeNotices(data.notices), eligible = takeNotices(data.ownNotices);
  hold?.reconcileUsage(eligible.map(n => n.id), all.map(n => n.id));
  usageNotices({ notices: eligible, push: true }, tell);
  usageNotices({ notices: all, push: false }, tell);
}

/** Ignore a snapshot overtaken by its connection ending or a newer limits/notice event. */
export async function catchUpUsage(signal: AbortSignal, revision: () => number, read: () => Promise<unknown>, apply: (data: Record<string, unknown>) => void): Promise<void> {
  const before = revision();
  const answer = await read() as { data?: Record<string, unknown> } | null;
  if (signal.aborted || before !== revision() || !answer?.data) return;
  apply(answer.data);
}

/** Read quota notifications even while the agents bridge is being restarted. */
export function watchUsage(tell: (notice: Notice) => unknown, hold: NoticeHold | null, signal?: AbortSignal): Promise<void> {
  let revision = 0;
  return followEvents<{ type: string; data?: Record<string, unknown> }>(async listening => {
    const reader = readServices(RUN);
    if (!reader) throw new Error("Box services are starting.");
    return askServices(reader, "api/kuvo/usage-events", { signal: listening });
  }, ({ type, data = {} }) => {
    if (type === "kuvo.limits.updated") for (const told of limitsFollowers) told();
    if (readUsageEvent(type, data, tell, hold)) revision++;
  }, signal, listening => {
    for (const told of limitsFollowers) told();
    const catchUp = () => catchUpUsage(listening, () => revision, async () => {
      const reader = readServices(RUN);
      if (!reader) return undefined;
      const reply = await askServices(reader, "api/kuvo/limits", { signal: AbortSignal.any([listening, AbortSignal.timeout(10_000)]) });
      if (reply.ok) return reply.json();
      await reply.body?.cancel();
      return undefined;
    }, data => usageSnapshot(data, tell, hold)).catch(() => {});
    void catchUp();
    const timer = setInterval(catchUp, 60_000);
    timer.unref?.();
    listening.addEventListener("abort", () => clearInterval(timer), { once: true });
  });
}

export function readUsageEvent(type: string, data: Record<string, unknown>, tell: (notice: Notice) => unknown, hold: NoticeHold | null): boolean {
  if (type === "kuvo.usage.notices") {
    if (data.push === false) hold?.reconcileUsage([], takeNotices(data.notices).map(n => n.id));
    usageNotices(data, tell);
    return true;
  }
  if (type === "kuvo.limits.updated" && Array.isArray(data.ownNotices)) {
    usageSnapshot(data, tell, hold);
    return true;
  }
  return false;
}
