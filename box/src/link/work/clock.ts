// Daily work limits use midnight in your time zone.

import { yourZone } from "../../shared/zone";

/** Your time zone, if the box's setting (or `KUVO_TZ`) names one this runtime knows; this
 * computer's otherwise (undefined). */
export function zone(): string | undefined {
  return yourZone();
}

/** The wall clock at `at`, in your zone. */
export function wall(at: number, timeZone = zone()): { year: number; month: number; day: number; hour: number; minute: number; second: number } {
  const parts = new Intl.DateTimeFormat("en-US", { timeZone, year: "numeric", month: "2-digit", day: "2-digit", hour: "2-digit", minute: "2-digit", second: "2-digit", hourCycle: "h23" }).formatToParts(new Date(at));
  const part = (type: string) => Number(parts.find(each => each.type === type)?.value ?? 0);
  return { year: part("year"), month: part("month"), day: part("day"), hour: part("hour"), minute: part("minute"), second: part("second") };
}

/** The moment a wall-clock time is in your zone (days past a month's end roll on). */
export function instant(year: number, month: number, day: number, hour = 0, minute = 0, timeZone = zone()): number {
  const wanted = Date.UTC(year, month - 1, day, hour, minute);
  let guess = wanted;
  for (let tries = 0; tries < 3; tries++) {
    const seen = wall(guess, timeZone);
    const shift = wanted - Date.UTC(seen.year, seen.month - 1, seen.day, seen.hour, seen.minute, seen.second);
    if (!shift) break;
    guess += shift;
  }
  return guess;
}

/** The midnight that starts the day `at` is in. */
export function startOfDay(at: number): number {
  const now = wall(at);
  return instant(now.year, now.month, now.day);
}
