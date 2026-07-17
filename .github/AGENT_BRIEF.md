# Agent brief — hourly self-improvement

You are improving **easypanel-cli**: a Rust CLI + full-screen ratatui TUI for managing
multiple [EasyPanel](https://easypanel.io) hosts. Read `README.md` and `CONTRIBUTING.md`
before anything else.

**Goal:** make this project scalable, credible, and genuinely useful to the GitHub
community. One meaningful improvement per run — quality over volume.

---

## Hard constraint: you have no live server

You have **no EasyPanel server, no credentials, and no way to obtain them.** Never
invent any.

This matters far more than it sounds. Almost every real bug in this project was found
by calling a live server, and **none** of them by unit tests:

| Bug | Why tests missed it |
|---|---|
| `updateSourceGithub` silently reset `autoDeploy` to `false` | Endpoint side effect, invisible to any mock |
| Load column always read `0.00` | `loadAvg` is a flat array of 3 strings, not a `[ts, value]` series |
| **Every** server error message was discarded | EasyPanel nests `message` under `json`; the code read the top level — and **the test passed**, because its mock used a shape the server never sends |

That last one is the lesson: **a test that encodes a wrong assumption is worse than no
test**, because it converts a bug into evidence of correctness.

Therefore:

- Do not change behaviour that only a live API can validate.
- Never write a mock asserting an API response shape you cannot verify.
- Never write "verified", "tested against the server", or "confirmed working" for
  something you did not actually run.
- If an improvement needs live verification, **open a GitHub issue describing it and
  stop.** Do not guess.

## The backlog — start here

**Work the top unfinished item.** Check whether it is already done before starting; if
it is, move to the next. These are all verifiable with `cargo` alone — no live server,
no excuses. You are expected to finish one per run.

1. **Shell completions.** Add `clap_complete`; generate bash/zsh/fish completions via a
   hidden `completions <shell>` subcommand. Document it in the README. High value for a
   CLI, pure codegen, fully testable.
2. **Man page.** Add `clap_mangen`, generate `easypanel.1`, ship it in the release
   tarball and install it in `install.sh`.
3. **`cargo audit`.** Add `cargo-audit` to CI as its own job; fix or document whatever
   it reports.
4. **Split `src/tui.rs`.** It is ~3,700 lines and holds the worker, the app state, every
   form, and every renderer. Split into a `tui/` module (`app.rs`, `worker.rs`,
   `form.rs`, `render.rs`) with **no behaviour change** — tests must pass untouched.
5. **`unwrap`/`expect` audit.** Find every one reachable from untrusted input (API
   responses, config files, `$EDITOR`, terminal size). Each either becomes a real error
   or gets a comment proving it cannot fire.
6. **`--json` output** for the read-only commands (`stats`, `monitor services`,
   `project list`, `domain list`, …) so people can script against it. Print the API's
   own JSON; do not invent a schema.
7. **Test coverage for `src/config.rs` and `src/output.rs` edge cases** — malformed
   JSON, missing file, unreadable permissions, empty series, huge byte values.
8. **Issue and PR templates** under `.github/`, plus README badges (CI, release,
   licence).
9. **`cargo publish` readiness** — `cargo package --list` clean, no stray files.

When the list is empty, propose the next item at the bottom of this file in your PR
rather than inventing work silently.

## Also in scope, any time

- **Docs accuracy.** A README that promises what the code does not do is a bug — it once
  claimed `stats` showed uptime, which nothing rendered.
- **Dependency updates**, CI improvements, packaging.
- **Contrast/accessibility** — named colours (`Color::Blue`) are reinterpreted by
  terminal themes and have twice produced unreadable text here. Use `Color::Indexed`.

## You are expected to ship

The constraints below are about **how** to work, not permission to skip working. "Do
nothing" is only correct when the whole backlog above is genuinely done — which today it
is not. Fourteen consecutive runs that produced nothing is a failure, not caution.

If you cannot push or open a PR, **say so loudly and specifically in your final
message** (the exact command and the exact error). Do not fail silently.

## Out of scope

Anything requiring a live EasyPanel host: new endpoints, changed request bodies,
response-shape assumptions, metrics joins. File an issue instead.

## Bug classes that keep recurring here — hunt these

1. **Filtered view vs. action index.** If a table filters what it renders but an action
   (`x`, `e`) indexes the *unfiltered* list, it hits the wrong row. Render and actions
   must share one filtered source.
2. **Silent defaults that change config.** A dropdown whose current value is missing
   from its option list must not jump to the first option — that silently changes what
   deploys. Preserve the current value.
3. **Unmodelled fields dropped on edit.** Edit forms must start from the original JSON
   so fields the form does not model (middlewares, `nixpacksVersion`) survive.
4. **Confidently wrong numbers.** A metric that reads `0.00` because of a shape
   mismatch is worse than a missing one. Prefer `-` over a fake `0`.
5. **Destructive actions without confirmation.** Deleting a server used to wipe its
   token instantly, with no prompt and no way to recover it.

## Do not trust your own harness

Measurement lied more often than the code did in this project's history:

- zsh's builtin `echo` interprets backslash escapes and **corrupts JSON** (`\n` inside a
  string becomes a real newline). This caused a false "the server returns malformed
  JSON" claim that reached a commit message.
- `tac` does not exist on macOS; a verification loop silently ran zero times and its
  empty output read as "passed".
- ratatui **diff-renders**: only changed cells are emitted. Screen-scraping a TUI for a
  count is unreliable — a stale title reads as a broken feature. Prefer unit tests over
  scraping.

When a check says something is broken, first ask whether the *check* is broken.

## Every change ships a release

Non-negotiable, per the project owner:

1. Update `CHANGELOG.md` under `## [Unreleased]` using
   [Keep a Changelog](https://keepachangelog.com) headings
   (Added / Changed / Fixed / Removed / Security).
2. When the change is complete and green, promote `[Unreleased]` to the new version,
   bump `version` in `Cargo.toml` (semver), and tag `vX.Y.Z`.
3. Pushing the tag triggers `.github/workflows/release.yml`, which builds binaries for
   linux-x86_64, darwin-arm64, and darwin-x86_64 and publishes the GitHub Release.
4. Release notes explain **what changed and why it mattered** — not a diff summary. If a
   bug could have destroyed data or shown a wrong number, say so plainly.

Trivial or no-op runs need no release. Do not invent work to justify one.

## Definition of done

- `cargo fmt --check` clean
- `cargo clippy --all-targets -- -D warnings` clean
- `cargo test` green
- CHANGELOG updated; release tagged if user-visible
- Commit message states what changed and **why**, and honestly names what was *not*
  verified
