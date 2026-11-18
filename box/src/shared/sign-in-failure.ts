/** What a client says of a sign-in that ended without signing in: `expired`, or `failed` with the
 * engine's `message` (a sign-in page that sends back an error hands the engine its OAuth code, as
 * `access_denied` when you turn it down there, which the engine passes on as it is). */
export function failureWords(status: "failed" | "expired", message?: unknown): string {
  if (status === "expired") return "The sign-in timed out. Try again.";
  const said = typeof message === "string" ? message.trim() : "";
  if (!said) return "Signing in didn’t work.";
  const lower = said.toLowerCase();
  if (lower.includes("access_denied")) return "The sign-in was turned down on its page. Try again to sign in.";
  if (lower.includes("server_error") || lower.includes("temporarily_unavailable")) return "The service couldn’t sign you in just now. Try again in a moment.";
  // The page's answer was for another try: an address pasted from an earlier sign-in.
  if (lower.includes("state mismatch") || lower.includes("state_mismatch")) return "That page was from an earlier sign-in. Try again to sign in.";
  // Another OAuth code, on its own: in words.
  if (lower === "invalid_grant") return "Signing in didn’t work (invalid grant).";
  if (["The sign-in was turned down on its page. Try again to sign in.",
    "The service couldn’t sign you in just now. Try again in a moment.",
    "That page was from an earlier sign-in. Try again to sign in.",
    "Signing in didn’t work (invalid grant).",
    "Claude Code's sign-in ended before it finished.", "The sign-in ended before it finished. Try again.",
    "That code didn't work. Start again, and paste the code from the page this sign-in opens.",
    "Claude didn't take that code. Start again for a new one.",
    "GitHub didn’t finish signing in. Start again for a new code."].includes(said)) return said;
  return "Signing in didn’t work.";
}
