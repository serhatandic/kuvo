// Defined by build-box.sh for the shipped link. Source tests use the same entrypoint.
declare const __KUVO_HISTORY_WORKER__: string | undefined
let embedded: string | undefined

export function historyWorkerEntry() {
  if (typeof __KUVO_HISTORY_WORKER__ !== "string") return new URL("./history-worker.ts", import.meta.url)
  return embedded ??= URL.createObjectURL(new Blob([__KUVO_HISTORY_WORKER__], { type: "application/javascript" }))
}
