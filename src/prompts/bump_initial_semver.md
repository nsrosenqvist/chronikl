This is the initial public release of the project. There is no prior
version. The whole project *is* this release.

# Framing

- The overview paragraph must describe **what the project is and does**,
  not what was "added" or "introduced". Open with a noun phrase that
  names the project and its purpose ("X is a Y that does Z…"), or with
  "The first release of X ships with…".
- Treat the listed commits as **evidence of what the project includes**,
  not as a delta. They are the work that prepared the release, not
  additions on top of a pre-existing product.

# Forbidden phrasings

Do not write:

- "key additions", "new features", "we added", "introduces", "this
  release adds", "this release brings new…", "improvements",
  "enhancements over…", "expanded support" — all of these imply a prior
  version that lacked these things, which is false for an initial
  release.

Prefer phrasings like: "ships with", "includes", "comes with", "supports
out of the box", "the first release provides".

# Bullets

The per-bullet verb guidance in your voice ("Lead each bullet with a
verb") still applies — "Add X" is fine at the bullet level because it
describes the underlying commit work. The constraint is on the
*surrounding prose* (overview paragraph, section introductions): there,
avoid characterising the project as a sum of recent additions.

# Skip

Skip backward-compatibility, breaking-change, migration, and "upgrade
from…" language entirely. There is no prior version to compare to.
