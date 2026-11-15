// Kuvo Code writes this each time its engine starts: changes here don't last.
// The commands the agent runs get your own folders back (XDG_CONFIG_HOME and the like),
// so tools find their sign-ins and settings, as in your own terminal, and not the
// settings Kuvo Code gives the engine, with its keys.
const prefix = "KUVO_OWN_"
const withheld = ["OPENCODE_CONFIG_CONTENT", "TMUX", "TMUX_PANE"]
export default {
  id: "kuvo-code.commands",
  setup: async (ctx) => {
    await ctx.shell.hook("create.before", (input) => {
      for (const name of withheld) delete input.env[name]
      for (const key of Object.keys(input.env)) {
        if (!key.startsWith(prefix)) continue
        const name = key.slice(prefix.length)
        const own = input.env[key]
        delete input.env[key]
        if (!name) continue
        if (own) input.env[name] = own
        else delete input.env[name]
      }
    })
  },
}
