# Marketplace starter catalog

For approval on 23 November 2026. The catalog is free. Entries are in `catalog/`, and the box builds from the same snapshot its public refresh will read.

| Apps | What people can use |
| --- | --- |
| GitHub | Repositories, pull requests and issues. Connect with a personal access token. |
| Linear | Issues, projects and cycles. |
| Notion | Pages and databases. |
| Sentry | Issues and application performance. |
| Atlassian | Jira issues and Confluence pages. |
| Stripe | Payments, customers and documentation. |
| Supabase | Database, auth and storage tools. |
| Neon | Postgres databases and branches. |
| Cloudflare Docs | Public documentation search. |
| Hugging Face | Models, datasets, Spaces and papers. |
| Slack | Search conversations and channels. The box hides it until the hosted sign-in is ready. |

The source of each mark is recorded in its app manifest. The marks identify third-party services.

| Agents | Job | Skill | Weekly routine, off until enabled |
| --- | --- | --- | --- |
| Coding Reviewer | Find regressions and report concrete findings. | Review changes | Check for changes awaiting review |
| Release Notes Writer | Draft user-facing notes from the chosen baseline. | Write release notes | Draft a release summary |
| Docs Keeper | Verify documentation against current behavior. | Check documentation | Check for documentation drift |
| Inbox Triager | Sort supplied messages and draft follow-ups. | Triage an inbox | Summarize open follow-ups |
| Research Assistant | Answer questions from primary sources. | Compare sources | Check for updates to tracked questions |
| Bug Triager | Group reports and identify the next action. | Triage bug reports | Summarize new bug reports |

Bug Triager requires Linear. The other five can start with the material the user gives them. Codex GPT-6.2 Sol is suggested, and the Add step asks which available account and model will run the bot.

The two other starter skills are **Summarize a meeting** and **Check a handoff**.
