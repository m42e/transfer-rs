---
description: "Use when generating release notes, changelogs, release summaries, or version announcements from commit history. Converts Conventional Commits into structured release notes."
name: "Release Notes From Conventional Commits"
---
# Release Notes From Conventional Commits

- When generating release notes from commit history, interpret commit summaries using the Conventional Commits 1.0.0 format.
- Ask for the release range, tag pair, or version boundary if it is not provided.
- Start with a short release title or summary, then organize the notes into sections derived from commit types.
- Prioritize these sections when they exist: `Breaking Changes`, `Features`, `Fixes`, `Performance`, `Documentation`, `Refactors`, `Tests`, `Build and CI`, and `Chores`.
- Surface breaking changes first, including the impacted scope and any visible migration concern when the commits make it clear.
- Convert terse commit summaries into reader-facing bullet points, but do not invent behavior, scope, or user impact that is not supported by the commits.
- Merge duplicate or closely related commits into a single concise bullet when that improves readability.
- Omit low-signal maintenance noise by default unless the user asks for a fully exhaustive changelog.
- Preserve important scopes when they help readers understand where the change landed, for example `cli`, `storage`, or `tui`.
- If the commit history alone is not enough to produce trustworthy notes, say what context is missing instead of guessing.