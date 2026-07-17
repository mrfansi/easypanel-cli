# Agent brief — hourly self-improvement

You are improving **easypanel-cli**: a Rust CLI + full-screen ratatui TUI for managing
multiple [EasyPanel](https://easypanel.io) hosts. Read `README.md` and `CONTRIBUTING.md`
before anything else.

**Goal:** make this project scalable, credible, and genuinely useful to the GitHub
community. One meaningful improvement per run — quality over volume.

---

## Ground truth from the server (verified 2026-07-18, EasyPanel 2.32.2)

The owner granted temporary read-only SSH to the running host and the whole direction
was checked against it — not guessed. Keep this even after the access is gone:

- **Every endpoint this tool calls exists in EasyPanel 2.32.2's `backend.js`.** The
  endpoint usage is correct and current, not reverse-engineered wishful thinking.
- **EasyPanel runs on Docker Swarm.** A "service" is a swarm service; deploy is a swarm
  update + image build, which is *why* `deployService` blocks for minutes and why create
  must not deploy inline. The dispatch-and-don't-wait approach is right.
- **Logs are backed by Grafana Loki + Promtail** (containers on the host). That is why
  `queryServiceLogs` returns Loki-shaped `{entries:[{values:[[ns,line]]}]}` and why the
  live tail with a nanosecond `start` cursor is the correct model.
- **`enabled` is a config flag, not a running state.** Ground truth for "is it running"
  is whether the container is `Up` — which the API exposes only as "does it have metrics
  in `getAllServicesStats`". That is exactly what the Status column now uses (v0.15.0).

### When the owner provides SSH (read-only only)

If a run has SSH to the host, it may **read** to validate direction — `docker ps`,
EasyPanel version, `backend.js` endpoint names, container state. **Never mutate anything
on the server over SSH**: no restarts, no writes, no touching real services or the data
volume (it is LMDB at `/etc/easypanel/data/`, and it holds every service's secrets — do
not dump values). All mutations still go through the API against a `zzz-*` throwaway.
Treat the key like any credential: never print it, never commit it.

## Hard constraint: you have no live server (cloud runs)

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
12. ~~**`cargo publish` readiness.**~~ Done. `Cargo.toml` `exclude` drops the 0.7 MB
   `easypanel-api.json` (developer reference, never read at runtime — only mentioned in a
   comment), `install.sh`, and the `.github/`/editor meta. The package went from 34 files
   / 1.2 MB to 23 files / 442 KB, all `src/` + README/LICENSE/CHANGELOG. `cargo publish
   --dry-run` compiles the packaged crate standalone; the name `easypanel` is free on
   crates.io. Actual publishing is left to the owner — it is public and irreversible, so
   it is a deliberate `cargo publish`, not something an agent should do unprompted.

**The original hardening backlog is complete.** The mandate now is explicit and
standing: **keep making this a richer, more scalable tool** — the owner asked for it.
Every run should leave the project able to do something useful it could not do before,
or handle scale it could not handle before. Still one meaningful, verified change per
run; still no invented features and no unverified claims.

### Killer feature DONE (v0.18.0): remote container terminal

The protocol is **fully verified with a live WebSocket round-trip** (2026-07-18), not
guessed. A shell into any container, on any host, from the TUI — using the API token the
tool already stores. Nothing to reverse-engineer; just build it.

- **URL**: `wss://{panel}/ws/containerShell?container={id}&command={b64}&token={apiToken}`
  (Fastify route under prefix `/ws`; the panel's http(s) origin → ws(s)).
- **Auth**: the same API token, as the `token` query param. No session cookie needed.
- **Container id**: resolve first via `projects/getDockerContainers` with
  `{ "service": "{projectName}_{serviceName}" }` → item's `Id` (and `State` == "running").
- **command**: base64 of a shell, e.g. `sh`. Runs `docker exec -it <id> /bin/sh -c sh`.
- **Framing (JSON both ways)**: client→server `{"input":"<keys>"}` and
  `{"resize":[cols,rows]}`; server→client `{"output":"<bytes>"}`. Shell exit closes the
  socket.

Build notes: add a blocking WebSocket client (`tungstenite` with rustls, matching the
existing rustls stack). Reuse the `$EDITOR` hand-off pattern in `mod.rs` — release
ratatui (`ratatui::restore()`), go raw, run the session, re-acquire (`ratatui::init()`).
One thread pumps WS `output` → stdout; the main loop reads raw stdin → `input`; send an
initial `resize` from the real terminal size. Verified live: connecting with
`command=base64("sh")`, sending an `input` of `echo …`, the `output` came back with the
shell prompt and the command result. This is the flagship feature — do it well.

### Other killer features — what the server's reality says matters most

Prioritised from a read-only look at the live host (42 swarm services, Grafana Loki
logs, real services crash-looping — `harisenin-com_webapp` had `Exited (1)`). These turn
the tool from "a nicer panel" into "the first place you look when something breaks."
Each is grounded in an endpoint confirmed to exist in `backend.js` 2.32.2.

1. ~~**Cross-service log search.**~~ Done in v0.17.0. `g` → query → parallel fan-out of
   `queryServiceLogs` (with `search`) across every service, grouped results. Verified
   live: "Error" across 33 services in 0.5 s. The highest-leverage feature, shipped.
2. **Health & crash visibility.** On a real host, services crash and swarm restarts them
   — the operator needs to *see* it. The Status column already separates running from
   stopped (v0.15.0); go further: surface unhealthy/recently-crashed/restart-looping
   services at the top, with exit reason and restart count. Data sources to verify:
   `projects/getDockerContainers` (takes `service`), `monitorOld/getDockerTaskStats`.
   Make the tool answer "what's broken right now?" at a glance.
3. **Alerting.** Wire crash/health detection to EasyPanel's own notification channels
   (`notifications/createNotificationChannel`, `sendTestNotification`). A terminal tool
   that can also notify is a real operational upgrade.

### Growth backlog — richer features (every endpoint verified in 2.32.2)

Fill out the management surface. Verify live with a `zzz-*` target and clean up.

4. **Port management.** *Add* done in v0.16.0. Open: **delete** (`deletePort` by index)
   and a **Ports step in the create wizard** (`createService` takes `ports` inline).
5. **Mount management** (`mounts/createMount`, `updateMount`, `deleteMount`) — TUI viewer
   is read-only; the CLI already has `mount-add`.
6. **Resource limits** (`updateResources`) — CPU/memory per service, TUI + wizard step.
7. **Templates** (`templates/createFromSchema`) — EasyPanel's one-click app catalogue;
   scope a slice (list + deploy one) first.
8. **Middlewares editor** (`middlewares/listMiddlewares`, `createMiddleware`) — the group
   is real (4 endpoints); domains preserve the field but can't edit it.
9. **Cloudflare Tunnel** (`cloudflareTunnel/*`, 11 endpoints) — expose services without a
   public IP; large but high-value for self-hosters.
10. **Basic auth** (`updateBasicAuth`), **redirects** (`updateRedirects`),
    **maintenance mode** (`updateMaintenanceMode`), **project env** (`updateProjectEnv`)
    — small self-contained editors.

### Scalability — the tool must not fold at real scale

The owner runs hosts with hundreds of services and 700+ domains. Things to watch and
improve, each verifiable:

- **Render cost** on huge tables (the flat services list, 713-domain hosts) — measure,
  don't guess; ratatui redraws every frame.
- **API round-trips** — the poll lane refetches everything every 2 s. Consider what can
  be incremental (logs already are) without going stale.
- **Many-host workflows** — the Hosts screen fans out per host; make sure a slow/dead
  host never degrades the rest (it currently does not — keep it that way as features
  grow).

### Also valid, any run

- **A real bug or rough edge found by driving the live TUI** — still the richest source;
  almost every important fix here came from watching the screen, not from tests. Prefer
  this over the backlog when you find one.
- **Publishing** to crates.io once the owner approves (readiness is done).

Do not invent filler. If a run genuinely finds nothing worth shipping, the correct move
is to do nothing and say so. A run that ships no code because there was nothing worth
shipping is a success,
not a failure — the earlier "fourteen empty runs" problem was avoidance, not restraint.

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

## UX, workflow, and architecture are part of "done"

Required by the project owner. A feature that works but is awkward is not finished.

- **UI/UX is a first-class concern.** Look at what you built the way a new user would.
  Is the important thing readable, or crammed and truncated (a result message pushed off
  the right edge behind the keybindings was a real bug)? Does a colour carry meaning?
  Does the layout waste or hide space?
- **The workflow must be seamless, following real best practice.** A user should move
  through a task without dead ends, silent no-ops, or a form they can't escape. Follow
  the pattern the domain already sets — for service creation, that is the EasyPanel
  dashboard's own flow (Basic → Source → Build), not an order invented here.
- **Navigation and layout must be clear.** Every screen says where you are and how to
  leave. A multi-step flow shows the step ("2/3 Source") and which key advances or goes
  back. No guessing which key does what.
- **Use established patterns in the architecture so it stays maintainable.** Reach for a
  known shape (a wizard is steps + next/back, a form field carries its own visibility
  and step rules) rather than a bespoke tangle. The test of a good structure: a new
  field or step is a small, obvious change — not an edit in five places. Keep the module
  boundaries the split established (`worker` I/O, `app` state, `keys` actions, `form`/
  `table` vocabulary, `render` draws nothing-decides).

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
