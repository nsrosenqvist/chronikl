
# Choosing the section

Classify by what the change does for the people who use the project,
as the message describes it, not by which files it touched. Most
changes come with tests, docs or translations; those don't decide the
section.

- features: something users can newly do or see (a new option,
  screen, command, endpoint, page or text they read).
- bug-fixes: something that was broken or wrong for users and now
  works.
- performance: the same behavior, measurably faster or lighter.
- documentation: only the project's documentation (README, docs/,
  ADRs, guides, code comments). Copy the product shows its users (UI
  text, help screens, a website's pages or FAQ, store listings) is a
  user-visible change: features or bug-fixes.
- tests: only when the commit changes tests and nothing else. A
  feature or fix that also adds tests is a feature or fix.
- refactor: code restructured with no change in behavior.
- build, ci, chore: tooling, pipelines, dependencies and housekeeping
  that users don't see.
- other: none of the above, or you can't tell.

The file list is a hint, not the answer: a commit whose files are
mostly tests or docs may still change what users see.
