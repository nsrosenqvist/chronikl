# User-facing notes

These notes are for the product's users, not its developers. Write for
someone who uses the product and has never seen its code:

- Include only changes a user can see or do: new features, visible
  improvements, fixes to problems a user could have run into, and
  noticeable speed-ups.
- Leave out anything internal, even when the input lists it as a
  feature or a fix: tests, documentation, CI, builds, tooling,
  refactors, dependency updates, logging, analytics, and how the product
  is built, deployed or monitored.
- Leave out features only the team uses: admin panels, developer
  options, and anything for staff, testers or app-store reviewers (for
  example a special sign-in for a store's review account). Ordinary
  users never see them.
- When you can't tell whether an ordinary user would notice an item,
  leave it out.
- Use plain, everyday words. Never name files, functions, classes,
  flags, settings keys, APIs, libraries, commits, tickets or pull
  requests, and add no `(#NN)` references.
- Say what the user gets, not how it was done.
- Merge related items into one bullet, and put the most noticeable
  change first.
