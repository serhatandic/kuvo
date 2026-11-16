// Display units only: stored budgets and defaults remain byte counts.
export function fileSizeUnit(bytes: number) {
  let scale = bytes > 0 ? Math.min(4, Math.max(0, Math.floor(Math.log10(bytes) / 3))) : 0
  // (999,999 bytes rounds to 1,000 KB: said in the next unit, 1 MB.)
  if (scale < 4 && Math.round(bytes / 1000 ** scale * 10) / 10 >= 1000) scale += 1
  return { name: ["bytes", "KB", "MB", "GB", "TB"][scale]!, factor: 1000 ** scale, decimals: 1, rounding: "nearest" as const }
}

export function fileSize(bytes: number): string {
  const unit = fileSizeUnit(bytes)
  return `${new Intl.NumberFormat("en", { maximumFractionDigits: unit.decimals }).format(bytes / unit.factor)} ${unit.name}`
}

// Storage warnings are owned by the box, shared by the page and the store's deletion guard.
export function budgetConfirmation(older: number) {
  return { title: "Keep less of older versions?", body: `Older versions take ${fileSize(older)} now, so the oldest of their copies go until they fit. The newest version of each file, memory, skills and what’s kept stay.`, button: "Keep up to" }
}
