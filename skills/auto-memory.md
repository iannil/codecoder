---
name: auto-memory
description: >
  After completing a milestone, write memory entries documenting project knowledge
  discovered during the milestone. Ensures knowledge accumulates across sessions.
  Use when prompted after milestone acceptance, or manually when you need to
  persist project learnings.
---

# Auto-Memory: Project Knowledge Accumulation

After completing each milestone, write a `memory/auto-<topic>.md` entry documenting
what you learned about the project during this milestone. This ensures knowledge
accumulates across sessions.

## When to write

After a milestone passes acceptance (verdict == pass), write one or more memory
entries covering:

1. **Codebase patterns discovered**: naming conventions, file organization,
   architectural patterns you observed
2. **Pitfalls encountered**: bugs, gotchas, things that went wrong and why
3. **Design decisions made**: why you chose approach A over B
4. **New dependencies or tools used**: what they do and how to use them

## Format

Each memory entry is a file under `memory/` named `auto-<kebab-case-topic>.md`:

```markdown
---
name: auto-<topic>
description: <one-line description>
metadata:
  type: project
---

<detailed content, 2-5 sentences>

**Why:** <why this knowledge matters>
**How to apply:** <how to use this knowledge in future work>
```

## Constraints

- Keep entries concise (2-5 sentences each)
- Only write entries for genuinely non-obvious knowledge
- Skip entries for things already documented in ADRs, ARCHITECTURE.md, or README.md
- Use the `memory` tool to write the entry