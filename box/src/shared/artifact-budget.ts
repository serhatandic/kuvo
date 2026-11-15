// Storage warnings are owned by the box, shared by the page and the store's deletion guard.
export function budgetConfirmation(older: number) {
  const units = ["bytes", "KB", "MB", "GB", "TB"]
  const scale = older > 0 ? Math.min(4, Math.floor(Math.log10(older) / 3)) : 0
  const size = `${new Intl.NumberFormat("en", { maximumFractionDigits: 1 }).format(older / 1000 ** scale)} ${units[scale]}`
  return { title: "Keep less of older versions?", body: `Older versions take ${size} now, so the oldest of their copies go until they fit. The newest version of each file, memory, skills and what’s kept stay.`, button: "Keep up to" }
}
