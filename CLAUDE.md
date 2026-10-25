## Agent skills

### Issue tracker

Local markdown under `docs/plans/` (gitignored, private). See `docs/agents/issue-tracker.md`.

### Triage labels

Default five roles, written as a `Status:` line in each issue file. See `docs/agents/triage-labels.md`.

### Domain docs

Single-context: root `GLOSSARY.md` + `docs/adr/`. See `docs/agents/domain.md`.

## Comments

Applies to all code.

- State what the code guarantees or why it is shaped this way. Nothing else.
- Technical facts only: no opinions, no "simply", "just", "clever", "we".
- One line where possible; a doc comment is at most two sentences.
- No restating the code, no history, no references to reviews, chats or tickets.
- Spec references only as section ids when the reason lives there.
