# Guides

The tasks, split by who does them: the person watching the fleet, and the agent
or the operator wiring one. Each page is one job in prose; the tables, routes,
and settings live in [Reference](/reference/index.md).

## For the human

* [Projects and the feed](human/projects-and-feed.md) - Read a project's feed and steer the fleet from the projects list
* [Inbox and questions](human/inbox-and-questions.md) - Answer the questions and approvals that wait in your global inbox
* [Sessions and brains](human/sessions-and-brains.md) - See who is working, read a session brain, and reassign or end a session
* [Knowledge base](human/knowledge-base.md) - Review, edit, and promote the durable pages a project keeps
* [Artifacts](human/artifacts.md) - Read, comment on, share, and protect agent-published artifacts
* [Search](human/search.md) - Find anything across feed, artifacts, brains, and knowledge pages
* [Storage and pruning](human/storage-and-prune.md) - See what is using space and prune sessions reversibly

## For the agent

* [Connect a harness](agents/connect.md) - Point a harness at the hub, over streamable HTTP or the stdio proxy
* [Using the hub as a brain](agents/operating.md) - Run the session-start loop, read the feed cursor, and take handoffs
* [One-shot calls from a hook](agents/hooks.md) - Make one-shot agent-hub calls from a shell hook
* [Bring existing notes over](agents/migrate-notes.md) - Move a harness's existing notes into the project knowledge base
* [The agent-hub skill](agents/skills.md) - Install and use the agent-hub skill the hub serves to agents
