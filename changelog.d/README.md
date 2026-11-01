# Changelog fragments

Don't edit `CHANGELOG.md` in a pull request. Add a fragment here instead, one
file per change, so parallel PRs never conflict on the changelog. The same
convention as whity-core and whity-id.

## Name

`<id>.<section>.md`, for example `WHIT-822.added.md`.

- **id** is the WHIT task, or anything unique (`WHIT-822-2` for a second entry
  from the same task, or the PR number for a change without a task).
- **section** is one of `added`, `changed`, `deprecated`, `removed`, `fixed`,
  `security`, as in [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## Content

One or more list items, written for the person reading the release page: what
changed and why it matters. No headings; the section comes from the file name.

CI runs `npm run changelog:check` on every PR, so a misnamed or empty fragment
fails there.
