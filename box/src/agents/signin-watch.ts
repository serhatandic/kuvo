import type { SignInAttempt } from "./agent"

/** Follows the attempt itself, even after its client stops asking for status. */
export function watchSignIn(attempt: SignInAttempt, expires: number, completed: () => void, every = 250): () => void {
  let stopped = false
  let timer: ReturnType<typeof setTimeout> | undefined
  const check = () => {
    if (stopped) return
    if (Date.now() >= expires) { stopped = true; attempt.cancel(); return }
    const status = attempt.status().status
    if (status === "complete") { stopped = true; completed(); return }
    if (status !== "pending") { stopped = true; return }
    timer = setTimeout(check, every)
    timer.unref()
  }
  check()
  return () => {
    if (!stopped && Date.now() < expires && attempt.status().status === "complete") completed()
    stopped = true
    clearTimeout(timer)
  }
}
