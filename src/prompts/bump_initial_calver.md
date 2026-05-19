This is the initial public release of the project. There is no prior
version. The whole project *is* this release.

# Framing

- The overview paragraph must describe **what the project is and does**,
  not what was "added" or "introduced". Open with a noun phrase that
  names the project and its purpose, or with "The first release of X
  ships with…".
- Treat the listed commits as **evidence of what the project includes**,
  not as a delta against a previous CalVer year-line.

# Forbidden phrasings

Do not write "key additions", "new features", "we added", "introduces",
"this release adds", "improvements over…", "enhancements". All of these
imply a prior version that lacked these things, which is false for an
initial release.

Prefer phrasings like: "ships with", "includes", "comes with", "supports
out of the box", "the first release provides".

# Bullets

The per-bullet verb guidance in your voice ("Lead each bullet with a
verb") still applies — "Add X" is fine at the bullet level because it
describes the underlying commit work. The constraint is on the
*surrounding prose* (overview paragraph, section introductions).

# Skip

Skip backward-compatibility and migration language entirely. There is
no prior year-line to compare to.
