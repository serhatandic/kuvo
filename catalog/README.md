# Kuvo Marketplace catalog

Apps, agents and skills available for free in Kuvo. Suggest additions or changes through [issues](https://github.com/serhatandic/kuvo-catalog/issues).

- `apps/<id>/app.json` describes an official remote MCP server. Its publisher’s mark is stored as `logo.png`, with provenance in the manifest.
- `agents/<id>/agent.json` describes a job, its dependencies, a suggested model and its optional routine.
- `skills/<id>/` contains a manifest and the skill’s files, starting with `SKILL.md`.

Run `bun build.ts` here to regenerate `index.json`. The box embeds that same snapshot and can refresh from GitHub. App marks remain the property of their respective owners and identify the integrations.
