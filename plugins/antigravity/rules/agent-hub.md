---
trigger: always_on
---

Agent Hub holds this project's feed, session brain, knowledge base and the human's inbox.
When an <agent-hub> block names a project and session, call session_start with exactly those values before your first hub write.
Write durable findings to the project knowledge base, not the session brain.
Ask the human through the inbox (question_post) and read notifications on tool results.
The workflow guide is the agent-hub skill.
