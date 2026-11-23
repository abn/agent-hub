---
trigger: always_on
---

Agent Hub holds this project's feed, session brain, knowledge base and the human's inbox.
Pass session: {"project_id": ..., "name": ...} from the <agent-hub> block on every hub call.
Write durable findings to the project knowledge base, not the session brain.
Ask the human through the inbox (question_post) and read notifications on tool results.
The workflow guide is the agent-hub skill.
