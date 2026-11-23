const readLimit = 30_000

/**
 * What a conversation's messages say, for a model to read. Cut to its newest part when long, with
 * `opening` (what you first asked, which may be older than `messages`) kept before it unless the
 * part kept has it.
 */
export function transcript(messages: any[], since: number, opening?: any): string {
  const lines: string[] = []
  const cut = (text: string, limit: number) => (text.length > limit ? text.slice(0, limit) + " […]" : text)
  for (const message of messages) {
    if ((message?.time?.created ?? 0) < since) continue
    switch (message?.type) {
      case "user":
        lines.push(`Person: ${cut(String(message.text ?? ""), 4_000)}`)
        break
      case "synthetic":
        lines.push(`Note: ${cut(String(message.description ?? message.text ?? ""), 600)}`)
        break
      case "assistant":
        for (const part of message.content ?? []) {
          if (part?.type === "text" && part.text?.trim()) lines.push(`Agent: ${cut(part.text.trim(), 4_000)}`)
          if (part?.type === "tool") {
            const input = cut(JSON.stringify(part.state?.input ?? {}), 300)
            const output = (part.state?.content ?? []).map((item: any) => (item?.type === "text" ? item.text : "")).join("\n").trim()
            const error = part.state?.error?.message ?? (typeof part.state?.error === "string" ? part.state.error : "")
            lines.push(`Agent ran ${part.name} ${input} (${part.state?.status ?? "?"})${output ? `:\n${cut(output, 1_500)}` : ""}${error ? `\nError: ${cut(String(error), 500)}` : ""}`)
          }
        }
        if (message.error?.message) lines.push(`(The agent's reply failed: ${cut(String(message.error.message), 500)})`)
        break
      case "shell": {
        // One you ran yourself ("!npm test").
        const output = typeof message.output === "string" ? message.output : message.output?.output
        lines.push(`You ran: ${cut(String(message.command ?? ""), 300)}${typeof message.exit === "number" ? ` (exit ${message.exit})` : ""}${output ? `\n${cut(String(output), 1_500)}` : ""}`)
        break
      }
    }
  }
  let text = lines.join("\n\n")
  const first = opening?.type === "user" ? `Person: ${cut(String(opening.text ?? ""), 4_000)}` : undefined
  const firstShown = first !== undefined && messages.some((message) => message?.id === opening.id)
  if (text.length > readLimit || (first && !firstShown)) {
    // Cut to its newest part, but what you first asked for stays: it's what the work is for.
    const head = first ?? lines.find((each) => each.startsWith("Person: ")) ?? ""
    const tail = text.length > readLimit ? text.slice(text.length - (readLimit - head.length)) : text
    if (!tail.includes(head)) text = (head ? head + "\n\n" : "") + "[…earlier turns left out]\n\n" + tail
    else text = tail
  }
  return text || "(Nothing yet.)"
}
