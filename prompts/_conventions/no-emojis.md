# No emojis

Do not use emojis or other decorative non-ASCII glyphs in any
output produced during this session. This applies universally:

- **Files you write.** Source code, comments, doc strings, log
  strings, error messages, markdown documents, critique JSON
  bodies, plan files, analysis reports -- all stay ASCII.
- **Your chat responses.** Prose, headings, bullet lists,
  status updates, summaries, finding-marker lines. No
  checkmarks, crosses, sparkles, fire, rockets, brain, eyes,
  lightbulbs, warning signs, or section-divider glyphs.
- **Tool-call arguments.** `write_file` content, `edit_file`
  `new_string`, `search` patterns, etc.

ASCII alternatives that are fine:

- `[x]` / `[ ]` / `[-]` for checklists.
- `BLOCKER:` / `RESOLVED:` / `UNRESOLVED:` for findings (these
  are the gate-relevant markers anyway).
- `OK` / `FAIL` / `PASS` / `TODO` / `NOTE` / `WARNING` for
  status calls.
- `->` / `=>` / `<-` for arrows; `--` for em-dash.

Why this rule exists: emojis muddle grep, diff review, log
search, and downstream tooling that assumes ASCII; they render
inconsistently across terminals and editors; and they add
visual noise to artifacts that are read in serious technical
contexts (reviews, audits, regression triage). The chat panel
in particular is a working surface, not a chat app -- keep it
plain.

If a prior turn or inlined input already contains emojis,
don't propagate them: when you quote or paraphrase, strip the
glyphs.
