import "./safety"
import { afterAll, beforeEach, expect, test } from "bun:test"
import { existsSync, mkdirSync, readFileSync, readlinkSync, rmSync, writeFileSync } from "node:fs"
import { dirname, join } from "node:path"
import { ask } from "./route-answer"

// Your time zone on a machine's box (PROTOCOL.md, "Your time zone"), on this system as it is (run on
// macOS and on Linux): changed through the box's settings and written where kuvo-wake reads it,
// carried from what an older Mac gave the machine, taken from one of your other computers, and this
// computer's own clock read as the system has it now. The box's code is real, in the run's home.

const { engineHandler } = await import("../src/link/server/router")
const { BOTS, K, RUN, SESSION_WAKES } = await import("../src/link/home")
const { ZONE_FILE, applyZone, readZoneSetting, takeZone, zoneFilesFor, zoneMail } = await import("../src/link/zone/setting")
const { ownClock, yourZone } = await import("../src/shared/zone")
const { zone } = await import("../src/link/work/clock")

const noEngine = () => { throw new Error("The engine isn't running.") }
const handler = engineHandler(noEngine as any, { name: "devbox" }, noEngine as any)
const write = (file: string, text: string) => {
  mkdirSync(dirname(file), { recursive: true })
  writeFileSync(file, text)
}
const read = (file: string) => (existsSync(file) ? readFileSync(file, "utf8") : null)
const folder = join(BOTS, "bot_zonetest01")
const rowOf = (page: any) => page.groups.flatMap((group: any) => group.rows).find((row: any) => row.id === "time-zone")

const clear = () => {
  for (const file of [ZONE_FILE, join(BOTS, "kit.json"), join(SESSION_WAKES, "timezone"), join(SESSION_WAKES, "zoneinfo"), join(RUN, "launch.env")]) rmSync(file, { force: true })
  rmSync(folder, { recursive: true, force: true })
}
beforeEach(() => {
  clear()
  write(join(folder, "bot.json"), JSON.stringify({ kuvo: 1, id: "bot_zonetest01", name: "Scout" }))
})
// (The run's other files share this home: none of this is left for them.)
afterAll(clear)

test("this computer's own clock is the system's as it is now", () => {
  const was = process.env.TZ
  try {
    delete process.env.TZ
    let linked: string | undefined
    try { linked = readlinkSync("/etc/localtime").split("/zoneinfo/")[1] } catch {}
    expect(ownClock()).toBe(linked ?? Intl.DateTimeFormat().resolvedOptions().timeZone)
    process.env.TZ = "Asia/Tokyo"
    expect(ownClock()).toBe("Asia/Tokyo")
  } finally {
    if (was === undefined) delete process.env.TZ
    else process.env.TZ = was
  }
})

test("a machine's zone, changed through its settings, is what its bots, its conversations' wake-ups and its own clocks go by", async () => {
  const before = (await ask(handler, "GET", "kuvo/settings/general")).json
  expect(rowOf(before).value).toBe("own-clock")
  // (Nothing yours yet: a machine's bots go by its own clock, as before.)
  expect(zoneFilesFor().timezone).toBe(ownClock())
  const changed = await ask(handler, "PATCH", "kuvo/settings/general", { row: "time-zone", value: "America/Sao_Paulo", was: rowOf(before).revision })
  expect(changed.status).toBe(200)
  expect(rowOf(changed.json).value).toBe("America/Sao_Paulo")
  expect(read(join(folder, "timezone"))).toBe("America/Sao_Paulo")
  expect(read(join(SESSION_WAKES, "timezone"))).toBe("America/Sao_Paulo")
  expect([zone(), yourZone(K)]).toEqual(["America/Sao_Paulo", "America/Sao_Paulo"])
})

test("a machine with nothing from before keeps none as its round goes on, and tells your other computers none: never its own clock as yours", () => {
  applyZone()
  applyZone()
  expect(read(join(folder, "timezone"))).toBe(ownClock())
  expect(read(join(SESSION_WAKES, "timezone"))).toBe(null)
  expect(readZoneSetting()).toBe(null)
  expect(zoneMail()).toBe(null)
})

test("a machine updated from 0.7 keeps its Mac's zone as carried: told to no one, and giving way to the first pick your other computers tell it", () => {
  write(join(BOTS, "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], projects: [], timeZone: "Europe/Istanbul" }))
  expect(readZoneSetting()).toMatchObject({ zone: "Europe/Istanbul", n: 0, carried: true })
  expect(zoneMail()).toBe(null)
  // (Another computer's own clock is never told, so never taken: count 0.)
  expect(takeZone({ zone: "Asia/Kolkata", n: 0 })).toBe(false)
  expect(takeZone({ zone: "Asia/Kolkata", n: 1 })).toBe(true)
  expect(readZoneSetting()?.carried).toBeUndefined()
  expect(zoneMail()).toMatchObject({ zone: "Asia/Kolkata", n: 1 })
  expect(read(join(folder, "timezone"))).toBe("Asia/Kolkata")
  // A launch's KUVO_TZ only where there's nothing else.
  for (const file of [ZONE_FILE, join(BOTS, "kit.json"), join(SESSION_WAKES, "timezone"), join(SESSION_WAKES, "zoneinfo")]) rmSync(file, { force: true })
  write(join(RUN, "launch.env"), "PATH=/usr/bin\nKUVO_TZ=Australia/Sydney\n")
  expect(readZoneSetting()).toMatchObject({ zone: "Australia/Sydney", n: 0, carried: true })
})

test("a zone you pick on a machine that updated overnight is told to your other computers; its carried zone never was", async () => {
  write(join(BOTS, "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], projects: [], timeZone: "Europe/Istanbul" }))
  const row = rowOf((await ask(handler, "GET", "kuvo/settings/general")).json)
  expect(row.value).toBe("Europe/Istanbul")
  expect(zoneMail()).toBe(null)
  expect((await ask(handler, "PATCH", "kuvo/settings/general", { row: "time-zone", value: "Asia/Tokyo", was: row.revision })).status).toBe(200)
  expect(readZoneSetting()?.zone).toBe("Asia/Tokyo")
  expect(zoneMail()?.zone).toBe("Asia/Tokyo")
})

test("a pick made on a box reinstalled (or just set up), before it hears your other computers, wins over the older pick they still have", async () => {
  const row = rowOf((await ask(handler, "GET", "kuvo/settings/general")).json)
  expect((await ask(handler, "PATCH", "kuvo/settings/general", { row: "time-zone", value: "Asia/Tokyo", was: row.revision })).status).toBe(200)
  // Their pick, from before, counted on far past this box's own count: refused.
  expect(takeZone({ zone: "Europe/Berlin", n: 5 })).toBe(false)
  expect(readZoneSetting()?.zone).toBe("Asia/Tokyo")
  // A pick made after it, elsewhere: taken.
  expect(takeZone({ zone: "Europe/Berlin", n: zoneMail()!.n + 1 })).toBe(true)
  expect(read(join(folder, "timezone"))).toBe("Europe/Berlin")
})

test("a pick counts past what the box had and at least the time it's made, so of two picks made apart the later wins", async () => {
  const { pickCount } = await import("../src/link/zone/setting")
  // Made at 1000 ms on a box that had heard of a pick counted 5: 1000.
  expect(pickCount({ n: 5 }, 1000)).toBe(1000)
  // On a box that had a later one: one past it, whatever its clock says (behind, here).
  expect(pickCount({ n: 5000 }, 1000)).toBe(5001)
  // With none, the time it's made; and a later pick elsewhere, counted at its own time, wins.
  expect(pickCount(null, 1000)).toBe(1000)
  expect(pickCount(null, 2000)).toBeGreaterThan(pickCount({ n: 5 }, 1000))
})

test("a Mac of 0.7 still connecting, its clock moved (you travelled): the kit it writes again moves a machine's carried zone, for it alone, never a pick", () => {
  write(join(BOTS, "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], projects: [], timeZone: "Europe/Istanbul" }))
  expect(readZoneSetting()).toMatchObject({ zone: "Europe/Istanbul", carried: true })
  write(join(BOTS, "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], projects: [], timeZone: "America/New_York" }))
  expect(readZoneSetting()).toMatchObject({ zone: "America/New_York", n: 0, carried: true })
  expect(read(join(folder, "timezone"))).toBe("America/New_York")
  expect(zoneMail()).toBe(null)
  // A pick: the kit's next zone is only noted.
  expect(takeZone({ zone: "Asia/Tokyo", n: 2 })).toBe(true)
  write(join(BOTS, "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], projects: [], timeZone: "Europe/London" }))
  expect(readZoneSetting()).toMatchObject({ zone: "Asia/Tokyo", n: 2, kitSeen: "Europe/London" })
  expect(read(join(folder, "timezone"))).toBe("Asia/Tokyo")
})

test("group chats' meetings go by your zone: the box gives it to their bridge with the chats' settings, which a bridge that can't read the box's folder goes by", async () => {
  const { groupSettingsHere } = await import("../src/link/areas/group-settings")
  const { CONFIG_FILE } = await import("../src/link/areas/config")
  const { chatZone } = await import("../src/agents/groups/runner")
  // A machine with its words for bots and settings of its own, as group chats need them.
  const words = join(K, "bin", "bot-words.json")
  const hadWords = existsSync(words), hadConfig = existsSync(CONFIG_FILE)
  if (!hadWords) write(words, readFileSync(new URL("../src/shared/bot-words.json", import.meta.url), "utf8"))
  if (!hadConfig) write(CONFIG_FILE, JSON.stringify({ permissions: [] }))
  try {
    expect(groupSettingsHere()?.timeZone).toBeUndefined()
    takeZone({ zone: "Asia/Tokyo", n: 2 })
    expect(groupSettingsHere()?.timeZone).toBe("Asia/Tokyo")
  } finally {
    if (!hadWords) rmSync(words, { force: true })
    if (!hadConfig) rmSync(CONFIG_FILE, { force: true })
  }
  rmSync(ZONE_FILE)
  const { boxZoneIs } = await import("../src/shared/zone")
  // (As the bridge takes it from the box's group chats' settings: routes.ts, main.ts.)
  boxZoneIs("Asia/Tokyo")
  expect(chatZone({ timeZone: "Europe/Berlin" })).toBe("Asia/Tokyo")
  // (What the box gave this process is what its other clocks go by too.)
  expect(yourZone()).toBe("Asia/Tokyo")
  boxZoneIs(undefined)
  expect(chatZone({ timeZone: "Europe/Berlin" })).toBe("Europe/Berlin")
})

test("a kit moving a carried zone is applied at once, whoever reads it first (here, what's told to peers)", () => {
  write(join(BOTS, "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], projects: [], timeZone: "Europe/Istanbul" }))
  expect(readZoneSetting()?.carried).toBe(true)
  write(join(BOTS, "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], projects: [], timeZone: "America/New_York" }))
  expect(zoneMail()).toBe(null)
  expect(read(join(folder, "timezone"))).toBe("America/New_York")
})

test("a pick told with its rules passes them on, with this computer's own where it has them", () => {
  // (Rules this computer has of its own come first; those sent are kept only without: time-zone.test.js.)
  expect(takeZone({ zone: "Asia/Tokyo", n: 1, zoneinfo: Buffer.from("TZif sent").toString("base64") })).toBe(true)
  expect(read(join(folder, "timezone"))).toBe("Asia/Tokyo")
  const told = zoneMail()
  if (existsSync("/usr/share/zoneinfo/Asia/Tokyo")) expect(told?.zoneinfo).toBe(readFileSync("/usr/share/zoneinfo/Asia/Tokyo").toString("base64"))
  else expect(told?.zoneinfo).toBe(Buffer.from("TZif sent").toString("base64"))
})

test("the zone a machine carried from before, picked as it is, becomes yours and goes to your other computers", async () => {
  write(join(BOTS, "kit.json"), JSON.stringify({ kuvo: 1, personalities: [], projects: [], timeZone: "Europe/Istanbul" }))
  const row = rowOf((await ask(handler, "GET", "kuvo/settings/general")).json)
  expect([row.value, row.line]).toEqual(["Europe/Istanbul", expect.stringContaining("Kept from what your Mac last gave")])
  expect(zoneMail()).toBe(null)
  const picked = await ask(handler, "PATCH", "kuvo/settings/general", { row: "time-zone", value: "Europe/Istanbul", was: row.revision })
  expect(picked.status).toBe(200)
  expect(rowOf(picked.json).line).toContain("on all your computers")
  expect(zoneMail()?.zone).toBe("Europe/Istanbul")
})
