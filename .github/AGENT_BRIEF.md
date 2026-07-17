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

1. ~~**Shell completions.**~~ Done — `completions <shell>` covers bash/zsh/fish/elvish/
   powershell, documented in the README, one test per shell.
2. ~~**Man page.**~~ Done — `man` renders `easypanel.1`; the release tarball ships it and
   `install.sh` installs it.
3. ~~**`cargo audit`.**~~ Done — CI runs it on every push and weekly on a schedule
   (advisories appear whether or not you commit). Reports 0 vulnerabilities today.
4. ~~**Upgrade ratatui 0.29 → 0.30.**~~ Done. Cleared the `lru` unsoundness and
   dropped `paste`; `cargo audit` went from 3 warnings to 1, and that one
   (`async-std`) is dev-only via `httpmock` and never ships. Required **no code
   changes** — the APIs this project uses were untouched by the major bump.
5. ~~**Configure a service while creating it, not after.**~~ Done in v0.10.0. `n` sends
   the source inline with `createService`. Field visibility now takes several conditions
   (service type AND source type), `Form::switch` is gone, and the source labels were
   renamed (`Source`, `Docker image`, `Registry user`, `Registry password`) so the merged
   form has no duplicate label for `by_label()`'s `find()` to resolve wrongly. Build/env/
   domains/ports/resources are still `createService`-capable and still not offered —
   propose them separately, one at a time.

6. ~~**Split `src/tui.rs`.**~~ Done in v0.11.1. 5,087 lines became `tui/` — `worker`
   (network, Req/Resp only), `app` (state + selectors), `keys` (a second `impl App`:
   key -> action), `form`, `table`, `render` (draws, decides nothing), `mod` (event loop,
   `$EDITOR` hand-off, the only holder of `ServerConfig`). No behaviour change: the same
   83 tests, untouched, and the test-name list diffed identical before and after. Largest
   file is now `app.rs` at ~960 lines. Tests deliberately stayed in one `tui/tests.rs` —
   an untouched suite is the proof; distributing them is separate work.
7. ~~**Dockerfile source type.**~~ Done in v0.12.0. `FieldKind::Editor` holds the
   contents and `Space` opens them in `$EDITOR`, reusing the env hand-off; the field
   shows a line count, not a snippet. Verified live: `createService` stores the inline
   Dockerfile byte-for-byte and `updateSourceDockerfile` persists an edit (it answers
   `200 {}` with no `json` key — the client already treats that as success).

   **Upload is the last one and is still not implementable.** `uploadCodeArchive` takes
   `archivePath`: a path that must already exist **on the server**. There is no upload
   endpoint anywhere in the API, so there is nothing to send a file to. Do not add a
   field that asks the user to type a server-side path — that is a guess, not a feature.
   If you find a real upload route, reopen this.
8. ~~**`unwrap`/`expect` audit.**~~ Done in v0.12.1. Every production `unwrap`/`expect`
   reachable from untrusted input was checked. One real bug: `mask_token` sliced the
   config token by byte (`&token[..6]`), so a hand-edited token with a multibyte char at
   the boundary crashed `server list` — the `len() <= 10` guard counts bytes, not chars.
   Now char-based, with a regression test. The two remaining production `unwrap`s
   (`confirm_key`, `render_picker`) were guarded by their callers; both were made total
   (let-else / if-let) so they cannot panic even if the call order changes. Everything
   else was already defensive: `output.rs` is `and_then`/`unwrap_or` throughout, layout
   `[n]` indices equal their constraint count, terminal arithmetic is `saturating_sub`,
   and API-driven indexing is guarded by `is_empty()`.
9. ~~**`--json` output.**~~ Done in v0.13.0. A global `--json` flag makes every
   read-only command print the API's raw response (pretty) instead of a table — the
   server's own shape, no invented schema, empty results as `[]` so `jq` works. One
   process-level flag in `output.rs` gates it, so no `json: bool` was threaded through
   ~16 signatures. Covers project list/inspect, stats, node list, monitor
   services/storage, domain list, service ports/mounts/domains/databases/backups/
   volume-backups, action list, certificate list, notification list. Verified live.
10. ~~**Test coverage for `src/config.rs` and `src/output.rs` edge cases.**~~ Done.
   Added `unreadable_file_errors_and_never_wipes` (permission-denied read must error, so
   the write path refuses instead of wiping every server — self-adapting: skips under
   root, where 0o000 is bypassed), `format_bytes_survives_extreme_and_invalid_input`
   (negative/NaN → "0 B", the KB boundary, and huge values that never overflow past TB),
   and `series_helpers_are_empty_safe` (empty/missing series → 0.0 / empty spark, single
   point → midline). Each was proven to FAIL when its guard is removed — not padding.
   Corrupt-file and missing-file were already covered.
11. ~~**Issue and PR templates + README badges.**~~ Done. Three badges (CI, latest
   release, licence) at the top of the README. Issue templates for bug and feature,
   plus a `config.yml` pointing "how do I…" to Discussions; the PR template. All shaped
   to this project's character rather than boilerplate: the bug template asks for the
   exact command and the verbatim server error (how nearly every real bug here was
   found), and the feature template asks whether the API actually supports it, because
   this tool never invents a schema. YAML validated; badge URLs return 200.
12. **`cargo publish` readiness** — `cargo package --list` clean, no stray files.

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

## Drive the TUI and look at it before you call anything done

Required by the project owner, and the record backs them up. Every one of these was
found by a **human looking at the screen**, never by a test:

| What the tests said | What the screen showed |
|---|---|
| 76 green | `Enter` on a dropdown could never save the form — "Service baru" for type `app` had **no way to submit at all** |
| A test named `empty_project_shows_no_metrics_not_negative_zero` passed | `-0.0 %` had been on screen for two releases |
| `destroy` returned OK | The deleted row stayed in the table until a manual refresh |

Unit tests here check *shapes*. They cannot see a form you cannot escape, a key that
does nothing, a column that never updates, or a 100-second wait with no feedback. So:

- **Run the binary.** Open the screen you changed, press the keys a user would press,
  and complete the whole workflow end to end — not just the one key you touched.
- **Ask about the round trip**, not the request: after a create/delete/toggle, does the
  table *show* it? Does the status line say something true?
- **Time it.** If an action takes more than a couple of seconds, the UI must say so
  before it starts. `createService` with a github source takes ~100 s; silence there is
  indistinguishable from a freeze.
- **Every field must be reachable and leavable.** Tab through the form; if focus can
  land somewhere the form cannot be submitted or cancelled from, that is a bug.
- Prefer a unit test on the row/body builder for the *assertion*, but only after you
  have watched the real thing work. A green suite is not evidence that the feature is
  usable.

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
