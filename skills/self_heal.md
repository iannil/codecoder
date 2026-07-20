---
name: self_heal
description: Fix problems in skill/capability files (missing frontmatter, broken structure). Use when the agent detects a malformed skill or capability manifest.
---

## Self-Heal Procedure

When a skill (.md) or capability (manifest.json) file appears malformed:

1. Read the file to identify the specific issue
2. For missing frontmatter: add `---\nname: <name>\ndescription: <diagnosis>\n---\n\n` prefix
3. For missing `name:` field in frontmatter: insert it after the opening `---`
4. For capability manifest errors: rewrite the JSON with correct schema
5. Write the corrected content back
6. Run `/reload` to register the changes