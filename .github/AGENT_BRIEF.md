# Agent brief — hourly self-improvement

You are improving **easypanel-cli**: a Rust CLI + full-screen ratatui TUI for managing
multiple [EasyPanel](https://easypanel.io) hosts. Read `README.md` and `CONTRIBUTING.md`
before anything else.

**Goal:** make this project scalable, credible, and genuinely useful to the GitHub
community. One meaningful improvement per run — quality over volume.

## The north star (owner, restated 2026-07-18)

**This tool exists to COMPLEMENT the EasyPanel web dashboard — to do the things it can't,
or can't do well.** An enhancement is NOT judged by "does the dashboard/API docs have this
feature"; it's judged by **"does it have real impact for an EasyPanel operator."** Do not
anchor on parity with the panel. The features that landed hardest were exactly the ones the
panel lacks:

- **Clone a service** (config, incl. a DB's replication `configFile`) — no clone in the panel.
- **Search a keyword across every service's log at once** — the panel is one service at a time.
- **A shell into any container, embedded in the pane** — the panel has none.
- **One-key DB shell** (`y`): `mysql`/`psql`/`mongosh`/`redis-cli` already logged in with the
  service's own stored credentials — you never type a password. Nothing like it in the panel.
- **All-hosts-at-once** view — the panel shows one server at a time.

So when picking work: ask "what would make an operator's day easier that they can't get from
the panel?", verify it live, and build it. Convenience, safety, and speed on real operational
tasks beat filling in a management-surface checkbox. (Still: verify live, no invented shapes,
one solid change per run.)

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

### Deploy timing and error shapes (verified 2026-07-19)

- **`deployService` blocks until the build finishes.** Measured 51 s for a trivial
  Dockerfile (`RUN sleep 50`); real services take far longer. The client's 30 s timeout
  therefore trips on essentially every real deploy, and a proxy in front gives up around
  125 s with a 524.
- **A dropped connection does NOT cancel the deploy.** Both halves proven: the TUI
  reported "Deploy … failed: error sending request" at 30 s, and `listActions` recorded
  the same deployment as `done` 50 s in, with the service `active`. Never treat a timeout
  or a gateway 5xx as a deploy verdict — it is the opposite of the truth, and the natural
  user response is to deploy again.
- **A real refusal is a 4xx**: deploying a service that does not exist answers
  `400 {"message":"Invariant failed"}`; a wrong service type answers `404 {"error":"Not
  found"}`. Those must keep surfacing. `client::gave_up_waiting` draws exactly this line.
- Deploying an app service with **no source at all** is NOT an error — it answers 200 in
  0.2 s.

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

### Killer feature DONE (v0.26.0): clone a service — not in the web panel

`c` on a service composes a copy from `inspectService` → `createService` (everything inline
except `source`, which would deploy) → `updateSource*` (app) / `updateAdvanced` (db). Config
only, no data, same project, no deploy. Verified live field-by-field for `app` and `mysql`
(image, env, credentials, and the replication `configFile` all matched). The user's driving
case is MySQL replica setup. If extending: an option to target a different project (mind the
new-project Docker-network race — create the project as a separate step first).

### Other killer features — what the server's reality says matters most

Prioritised from a read-only look at the live host (42 swarm services, Grafana Loki
logs, real services crash-looping — `harisenin-com_webapp` had `Exited (1)`). These turn
the tool from "a nicer panel" into "the first place you look when something breaks."
Each is grounded in an endpoint confirmed to exist in `backend.js` 2.32.2.

1. ~~**Cross-service log search.**~~ Done in v0.17.0. `g` → query → parallel fan-out of
   `queryServiceLogs` (with `search`) across every service, grouped results. Verified
   live: "Error" across 33 services in 0.5 s. The highest-leverage feature, shipped.
2. ~~**Health & crash visibility.**~~ Done in v0.19.0. The Status column now shows
   `turun` (red) for any service whose Swarm replicas are missing (`desired > 0`,
   `actual < desired`) — a crash / restart-loop — and counts them in the table title
   (`… · ⚠ 2 turun`). Derived from `monitorOld/getDockerTaskStats` (one call, all
   services), which distinguishes a crash from an intentional stop (`desired = 0` →
   `berhenti`) — the two used to be indistinguishable. Verified live: the `{project}_
   {service}` join matches the replica map for all 33 services, and a deliberately
   crash-looping throwaway reported `actual=0, desired=1` → `turun`, then cleaned up.
   **Optional depth — DISPROVEN, do not attempt as described (checked 2026-07-19).**
   The plan was to read exit reason and restart count from the per-service
   `getDockerContainers` `Status` string ("Restarting (1)…"/"Exited (1)…"). Verified
   live against a deliberately crash-looping `zzz-*` service: `getDockerTaskStats`
   correctly reported `{actual: 0, desired: 1}`, and `getDockerContainers` returned
   **`[]` — an empty list — for the whole crash loop**. On Swarm the task container is
   removed as soon as it exits and a replacement is scheduled, so there is no container
   to inspect at exactly the moment you want to inspect it. The `Status` string is only
   ever observable for a service that is already *healthy* ("Up 11 hours"), which is
   when nobody needs it.

   What DOES answer "why is it down", verified in the same run: **the logs**. Loki keeps
   the dead container's output (the test service's `FATAL: cannot reach database` came
   back across several restart attempts, each under its own `container_name`), and the
   Logs viewer already shows it. So the useful half of this item is shipped and the
   proposed half is not buildable. Do not add a drill-down that renders an empty list.

   Worth knowing, same check: a service can crash-loop while its deploy action reads
   `status: "done"`. Deploy success does not mean the service runs.
3. **Alerting — not buildable as an in-tool feature (checked 2026-07-18).** The
   `notifications/*` group is only channel CRUD + `sendTestNotification`; there is **no
   endpoint to send a custom alert/message**. EasyPanel fires notifications from its own
   events — this tool cannot inject "service X crashed" into a channel. So the crash→notify
   idea is dead through the API. What *is* possible and honest: a `watch` CLI subcommand
   that polls `getDockerTaskStats` itself and notifies via an external hook (OS notifier,
   webhook) — a separate feature, not "wire to EasyPanel channels". Channel management
   (list/create/delete + send test) is doable but thin. Don't build channel management and
   call it alerting.

### Growth backlog — richer features (every endpoint verified in 2.32.2)

Fill out the management surface. Verify live with a `zzz-*` target and clean up.

4. ~~**Port management.**~~ Add done v0.16.0, **delete** done v0.21.0 (in the Ports viewer,
   press `[0-9]` → confirm → `deletePort` by index; list reloads in place). Verified live
   that the index equals the listed position. **Open:** a Ports step in the create wizard
   (`createService` takes `ports` inline) — additive, low priority.
5. ~~**Mount management**~~ Add + delete done in v0.30.0, mirroring ports: `M` opens a
   form (volume/bind/file → `createMount`), and in the Mounts viewer (`m`) a digit
   `[0-9]` → confirm → `deleteMount` by index, list reloads in place. Verified live that
   all three `values` shapes create and that `deleteMount` index = listed position.
   **Open:** `updateMount` (edit an existing mount) — needs the current values to prefill;
   lower priority.
6. ~~**Resource limits**~~ TUI done in v0.20.0. `L` on a service opens a form (CPU/memory
   limit + reservation, cores/MB, `0` = unbounded) → `services/{type}/updateResources`;
   works for every service type. Verified live: the endpoint round-trips the exact decimal
   numbers the tool sends. **Open:** a resources step in the create wizard (createService
   takes `resources` inline) — same shape, additive.
7. **Templates — the valuable half is NOT in the API (checked 2026-07-18).** The only
   endpoint is `templates/createFromSchema`; there is **no `listTemplates`**. The one-click
   catalogue (Ghost, WordPress, …) lives in an external source (EasyPanel's templates repo /
   frontend CDN as TS files), not the backend API — so "browse catalogue + deploy" can't be
   built from the API. `createFromSchema` only deploys a schema the user supplies, which is
   niche without the catalogue. Like alerting: the useful part isn't exposed. Don't build a
   paste-a-schema box and call it templates. If a catalogue JSON feed is ever found, reopen.
8. **Middlewares — large, sprawling, low daily value; deferred (assessed 2026-07-18).**
   `createMiddleware` is a huge `anyOf` over the whole Traefik middleware catalogue
   (addPrefix, basicAuth, compress, headers, rateLimit, stripPrefixRegex, …), each with a
   different config shape, plus per-domain assignment — a big surface for advanced routing
   most users never touch. The common redirect need is a SEPARATE, simpler endpoint
   (`updateRedirects`), now shipped (see #10). If middlewares are built later, scope one
   concrete type end-to-end (form + assign to a domain), not the whole catalogue.
9. **Cloudflare Tunnel — NOT verifiable in a session without a Cloudflare token (checked
   2026-07-18).** `getConfig` returns `null` on the live host (no tunnel configured), and
   every operation (`listAccounts`/`listZones`/`listTunnels`/`createTunnelRule`) requires a
   Cloudflare **`apiToken`** as input — a credential this project doesn't have and shouldn't
   obtain. Building it means guessing the `setConfig`/`createTunnelRule`/account→zone→
   hostname flow with no way to verify — exactly what the "no live server" rule forbids. If
   a run is given a Cloudflare token against a throwaway zone, do it then; otherwise it stays
   an issue, not a guess.
10. Small self-contained editors:
    - ~~**Basic auth**~~ done in v0.31.0. `H` on a web service (app/box/compose/wordpress)
      → form (Username + Password, prefilled from current) → `updateBasicAuth`; empty both =
      remove protection. Verified live: set and clear both round-trip via `inspectService`.
    - ~~**redirects**~~ done in v0.32.0. `f` on a web service views its redirects (digit
      `[0-9]` deletes), `F` adds one (regex → replacement, 301/302, enabled). No per-item
      endpoint, so add/delete are read-modify-write on the full `redirects` array via
      `updateRedirects`. Verified live: two adds preserve each other and delete-by-index
      removes the right one.
    - **maintenance mode** (`updateMaintenanceMode`) — **wordpress-only** endpoint (checked);
      niche, skip unless a wordpress user needs it.
    - **project env** (`updateProjectEnv`) — `{projectName, env}`. Note: `inspectProject`
      does **not** return the current env at top level, so prefill needs another source
      (maybe `project.env`) — verify before building or it edits blind.

### ~~Open UX defect: the status line cannot tell "working" from "finished"~~

**Done.** Failures stopped fading in v0.45.1; in-flight tracking landed in v0.46.1. The
worker's user lane now counts what it is working on (`spawn_worker` takes an
`Option<Arc<AtomicUsize>>`; only the user lane is counted, because the poll lane refetches
every two seconds and would pin a spinner permanently). The App reads that counter, so the
spinner reflects real work and the fade cannot report "Ready" over a running request.
Verified on screen against a black-holed host: `⠧ Loading…` held for 27 s without fading,
then the timeout error appeared and stuck; on Dashboard, where refresh sends nothing, there
is no spinner and the notice still fades at six seconds.

No send site changed. Counting in the worker rather than at the ~68 `req.send()` calls kept
this to one file's plumbing. The original analysis below is kept because the rejected fix
is still the tempting one:

**There is no in-flight tracking.** The spinner and the "is something happening?" signal
are both derived from the status *string* ending in `...` (`app.rs`, `spinner()`), and the
6-second fade rewrites that string to "Ready" regardless of whether the request came back.
So a dispatched action reports completion it cannot know about. The worst case is
`maint:systemPrune` (`keys.rs`) — a host-wide, irreversible Docker prune whose only
feedback is `"Sending..."`; six seconds later the bar says "Ready" while the request is
still running, and re-running it is a plausible next move.

The obvious fix (don't fade a status ending in `...`) is WRONG and was tried and rejected
on 2026-07-19: `refresh()` sets `"Refreshing..."` even on `Screen::Dashboard` and
`Screen::Terminal`, which send **no request at all**, and the data handlers
(`Resp::Projects`, `Resp::AllServices`, `Resp::Stats`) never touch the status — so today
that message is cleared *only* by the fade. Make it sticky and every `r` leaves a spinner
running forever.

The honest fix is a pending-request count on `App`, incremented where a `Req` is sent and
decremented when its `Resp` arrives, driving the spinner and gating the fade. That is a
cross-cutting change to ~50 send sites — a refactor of its own run, not a rider on a
feature.

### UI/UX critique findings — status

From the 2026-07-19 critique pass. Fixed:

- ~~Failures fade away after six seconds~~ — v0.45.1.
- ~~Status reports "Ready" over an in-flight request; spinner inferred from text~~ — v0.46.1.
- ~~Hosts table needs 123 columns with no narrow fallback~~ — v0.47.0. Note the critic
  predicted zero-width columns; ratatui actually shrinks them proportionally, which is
  WORSE than it sounds: "29.8 GB / 59.0 GB" rendered as "29.8 GB", a figure that reads as
  complete. Verified on screen at 80 columns before fixing.
- ~~Hosts has no row action, so a DOWN host cannot be investigated~~ — v0.47.0, `Enter`.

Fixed since:

- ~~Viewer cuts long lines~~ — v0.48.0. Horizontal scroll (`viewer_hscroll`, ←/→),
  keeping the vertical arithmetic intact; `Wrap` would have broken the follow-tail maths.
  Driving it turned up something the critique missed: → was swallowed by the GLOBAL tab
  handler, so reaching for the rest of a cut line threw you out of the logs onto another
  screen. ←/→ are now intercepted for `Screen::Viewer` before that arm.
- ~~Viewer scrolls past the end~~ — v0.48.0, clamped on every path rather than only while
  following. `Home` now also returns to the left edge, and both are advertised.

Still open, in the order I would take them:

1. ~~**`render_actions` at 80 columns**~~ — fixed in v0.48.3. Target truncated to 20 chars
   ("harisenin-net-db/php" for phpmyadmin). Now drops Duration first and Age only when
   four columns no longer fit — a history screen that cannot say WHEN has lost its point,
   and how long it took is one keypress away in the detail. This was the third copy of the
   drop-columns rule, so it was extracted to `table::columns_that_fit` in its own
   behaviour-preserving commit first; that helper returns INDICES rather than a prefix
   count precisely so a middle column (Duration) can be the one to go.
4. ~~**Monitor tiles below ~100 columns**~~ — fixed in v0.48.2. Confirmed on screen at 80
   columns first: Disk read "199.9 GB / 784" (a total with no unit) and CPU read
   "16 cores — loa". Each sub-line now offers a ladder of forms and the renderer takes the
   widest that FITS — "199.9/784.8 GB" keeps both halves in 14 columns, and CPU falls back
   to the load average, then the core count, then nothing. Same principle as the Hosts
   table: a shorter true form beats a longer cut one.
5. ~~**Form instructions live in the fading status line**~~ — fixed in v0.48.4. A form now
   carries its own `note`, drawn on its bottom border, so it lasts exactly as long as the
   form does. Opening a form no longer writes to the status line at all, which is the
   stronger fix: there is nothing left there to go stale. The status line is back to being
   only a transient toast.

### Audit: `by_label` visibility-blindness — the class is CLOSED (2026-07-19)

v0.50.4 fixed a leak where a database service inherited env typed for an app, because
`by_label` returns a hidden field's value exactly as if it were on screen. Rather than
stop at the two reported callers, every conditional field was cross-referenced against
every reader. **No further instance exists.** Do not re-audit this, and do not "fix" the
sites below — each is safe for a stated reason:

- **Decided by their own switch** — the reader matches on the very field that controls
  visibility, so a hidden field is unreachable by construction: `source_body` on `Source`
  (Repo/Branch/Git URL/Ref/Dockerfile/Docker image/Registry user+password/Path),
  `build_body` on `Build` (all six types), `mount_body` on `Type` (Host path/Content/Name),
  `domain_body` on `Destination` (Server URL/Weight vs Project/Service/Protocol/Port/
  Destination path — and it `remove`s the other branch's key).
- **Explicit visibility check** — `service_extra` tests `form.visible()` before taking
  Database/User/Password/Root password/Image.
- **Guarded by nesting** — `.env file path` and `Create .env file` are only read inside
  `if let Some(env) = create_env(form)`, which now returns `None` off `app`.
- **Not actually conditional** — `SSL resolver` is unconditional (shown for both
  destination types), so reading it unconditionally is correct.
- **A different field of the same name** — the `Project` reads in `app.rs` belong to the
  clone/migrate forms, where `Project` has no `.when`; the conditional `Project` lives in
  the domain form, whose reader switches on `Destination`.

**Method warning, learned the hard way in this audit:** a first regex sweep reported
`SSL resolver` as conditional and nearly produced a "fix" for a non-bug. The pattern had
matched across field boundaries and attached a later field's `.when` to it. When a scan
implicates a specific line, open that line before believing it.

### UI/UX critique, round 5 (2026-07-19) — the uncovered areas

Round 4 named three areas it had NOT audited; round 5 was pointed at exactly those and
returned the best report of the five: three confirmed findings plus a long, specific
"checked and CLEAN" list. Fixed in v0.50.4.

- ~~**Environment and Domains leaked into a database's create request**~~ — `create_env`
  and `create_domains` read the form unconditionally while their two siblings
  (`create_source`, `create_build`) guard on `Kind != "app"`. `by_label` is
  visibility-blind, so filling Environment as an app, stepping back and switching to
  postgres carried it along — and the wizard collapses to ONE page for a database, so the
  field is never on screen again to clear.

  **The critic could not say whether the server rejects or accepts it. I checked: it
  ACCEPTS and STORES the env** (`services/postgres/createService` with `env` returned the
  service with `env: "BOCOR=ya"`). So this was a silently misconfigured database, not a
  confusing error — the worse of the two possibilities. `domains` on a database is
  accepted but ignored (no domain was created); guarded anyway.
- ~~**`r` on an action detail claimed "Refreshing..." and re-fetched nothing**~~ — an
  action detail has no `viewer_ctx`, and `refresh`'s Viewer arm had no `else`. A RUNNING
  deploy's log stayed frozen at first fetch, on the screen you open to watch it. `App`
  now remembers the action id.
- ~~**Backups rows led with an unusable cuid and had no header**~~ — nothing in the TUI
  can act on that id (no run, no delete, not a collection so no selection), and the CLI
  prints the same data under labelled columns. NOT verified on screen: no backup schedule
  exists anywhere on the live host, so this one rests on the format string and the CLI
  comparison.

**Explicitly checked and CLEAN by round 5** — skip these next time: build-step field
visibility vs `build_body`'s key table for all six build types; wizard values surviving
step navigation (`goto_step` never clears `value`); the Domains step for a database (all
`Domain *` fields are `.when("Kind","app")`, so a database form is genuinely one page);
`.env file path` on submit; label collisions (`Domain path` vs source `Path`);
`Domain port` dropping an unparseable value; action-detail rendering for
running/failed/killed and its long-log navigation; action-detail Esc target; Actions
column dropping.

### UI/UX critique, round 4 (2026-07-19)

All three findings confirmed on screen and fixed in v0.50.3. This round asked for at most
3 findings, CONFIRMED only, with an empty list stated as an acceptable answer — and the
report was much better for it: it named what it checked and found clean (Terminal at a
degenerate pane size, destructive-confirmation wording) instead of padding.

- ~~**A fresh collection inherited the previous selection**~~ — `Resp::Viewer` reset every
  viewer field except `viewer_row`, and render only seeded it when `None`, which is true
  once per process. Opening Mounts after leaving Ports on row 5 armed row 5 of the new list
  under `x delete`. Reset lives next to the other resets now.
- ~~**Wheel and `j`/`k` were dead in a collection**~~ — they wrote `viewer_scroll`, which
  the table view does not read. They move the selection now, like every other table.
- ~~**Menus offered what a service type cannot have**~~ — `net_menu` was ungated, so a redis
  service was offered Redirects and Basic auth, and Redirects even OPENED, showing an empty
  list under a footer saying `n add` for something structurally impossible. Gated by the
  same type lists the handlers already check.
- Also: `open_view` returned silently on a project header while the menu path said "Select
  a service first" — the same action answering differently depending on how you reached it.

**Answered the round-4 open question with the live API**: `listDatabaseBackups` on an app
service returns `[]`, not an error. The Backups entry on a non-database is harmless, so it
stays ungated.

**Method note worth keeping**: verifying finding 1 on screen appeared to FAIL twice before
it passed. Both times the cause was tmux — Escape had not actually left the viewer, so the
follow-up keys went somewhere unintended. The unit test was right the whole time. When a
screen check contradicts a passing test, check the navigation before you doubt the fix.

### UI/UX critique, round 3 (2026-07-19)

Fixed in v0.49.2:

- ~~**`[0-9] delete` was dead for digits 1-7**~~ — CONFIRMED on screen. They were global
  tab keys, so seven digits out of ten threw the user onto another tab while the viewer's
  own border advertised "[0-9] delete". Since each collection became a single screen this
  was its ONLY delete. Same collision and same fix as ←/→ before it: the viewer now owns
  its digits; Esc then the digit still switches tab.
- ~~**Silent misses in the viewer**~~ — a digit with no row behind it, and `a`/`e`/`b` in a
  viewer that does not take them, both did nothing at all. They now say what this screen
  accepts ("Not here — e edit") or that the row is absent.

Still open, with evidence, in the order I would take them:

1. ~~**Digit-delete has a hard ceiling at [9]**~~ and ~~**the three screens disagree on
   their verbs**~~ — both done in v0.50.0. A collection view is now a one-column table with
   a selected row (moved by the same `move_table` every other table uses), and `x` deletes
   it — the verb Domains and the server picker already had. `a` became `n` to match too.
   Verified live by deleting the twelfth of twelve mounts, which the digit scheme could
   never address.

   Two things that only showed up by running it: the index must come from the `[n]`
   PRINTED on the row, not the row's position (the view appended a blank line and a hint,
   so `End` selected a non-row and offered to delete `[13]` of 12); and those "Press a
   digit [0-9] to delete" hint lines were both wrong and redundant once the border carried
   the keys.
3. ~~**The server picker truncates the URLs that exist to disambiguate servers**~~ —
   CONFIRMED on screen and fixed in v0.50.1. At 46% of an 80-column terminal the box was 36
   wide: the title lost "x delete" and all three URLs were cut with no ellipsis. It is now
   sized from its content, the keys go through `fit_hints` (dropped whole, never cut), and
   a URL that still does not fit ends in "…".

   Note `centered_abs` takes a PERCENTAGE despite its name — that is how this happened.
   `centered_abs_w` is the real absolute-width helper.
4. ~~**Empty states / log-search feedback**~~ — checked on screen and settled in v0.50.2.

   The log-search half was WRONG: `log_search` already answers "No match for '<q>' in any
   service." with a title reading "0 lines in 0 services". That is the third speculative
   critique finding this session that did not survive being looked at — verify before
   fixing, always.

   The empty-state half was real, and one part of it was a regression from the previous
   run: converting a collection view to a table made the "No ports" PLACEHOLDER a
   highlighted, selectable row. It now reads as a message and says "press n to add one".
   Domains with nothing to show distinguishes "no domains yet" from "your filter excluded
   everything — Esc clears it", which need different actions.

**The round-3 critique list is now empty.** Screens have changed again since it was
written (collections became selectable tables, the picker was resized), so a fresh pass is
the reasonable next step — but note the hit rate: of five round-3 findings, three were
confirmed and worth fixing, one was disproven, and one was a duplicate of a fixed item.

### Audit: one door per thing (owner, 2026-07-19)

The owner asked why "View env", "Edit env (partial)" and "Replace entire env" were three
menu entries for what is one screen — and then asked for the same AUDIT across the rest.
Done in v0.49.0. The pattern was systemic:

| Collection | Doors before | After |
|---|---|---|
| Env | View · Edit (partial) · Replace entire | **Env** |
| Ports | View · Add | **Ports** |
| Redirects | View · Add | **Redirects** |
| Mounts | View · Add | **Mounts** |
| Source & build | View · Set source · Set build | **Source & build** |

17 service menu entries became 11. The viewer already deleted rows by digit, so it was
already the place you act on a collection — "Add X" was a second door into the same room.
It now also adds (`a`) and edits (`e`, plus `b` for build), routed through the SAME
`services_key` handlers the menu used, so there is no second path to drift.

Two labels were also lies: saving sends the whole `env` string, so "Edit env (partial)"
was a full replace, and "Replace entire env" differed only in opening `$EDITOR` blank —
something you do inside your editor. The blank-editor mode and its `w` key are gone.

**When adding a feature, check this first:** does it need a new door, or does it belong on
the screen that already shows the thing?

### UI/UX critique, round 2 (2026-07-19) — unverified findings

From a fresh critique pass over the screens the earlier sweep skipped. **I have not
verified these on screen yet** — the critic could not run the binary, and its predictions
have been wrong before (it said the Hosts columns would render at zero width; they
actually shrank proportionally, which was worse). Confirm each one on screen before
fixing it.

1. ~~**Create-service wizard defers all validation to the last step**~~ — CONFIRMED on
   screen and fixed in v0.48.7. Worse than reported: it also walked past step 1 with an
   EMPTY Name, and the eventual message ("Service names may only contain a-z, 0-9, - and
   _") blamed the character set of a name that was simply missing. Each step is now
   validated on the way OUT, so a refusal can never surface two steps from its field, and
   it is drawn on the form's own border rather than the fading status line.
2. ~~**Monitor's filter is half-wired**~~ — CONFIRMED and fixed in v0.48.9, and worse
   than reported: navigation counted raw METRIC entries, which excludes the project header
   rows the table inserts, so with 60 metrics across 11 projects the table drew 71 rows and
   the cursor stopped at 60 — the last eleven unreachable with NO filter involved. Three
   call sites computed this count independently and all three disagreed; there is now one
   `App::monitor_rows_shown()`. Storage also gained `visible_storage_rows()` and a
   `count_title`, so `/` there both works and shows itself.
3. ~~**The Chooser dropdown closes silently when nothing matches**~~ — CONFIRMED on screen
   and fixed in v0.48.11. Enter closed the dropdown, left the field on its old value and
   said nothing, which is indistinguishable from a successful pick. It now stays open, the
   empty box reads "nothing matches — Backspace to widen", and the keys are on its border.
4. ~~**The Domains table still uses `Percentage` constraints**~~ — CONFIRMED and fixed in
   v0.48.10. At 80 columns a source rendered as "https://harisenin-net-db-mysql-m" with no
   ellipsis, next to `x delete`. Now uses `columns_that_fit` (ID dropped first, Source never
   dropped) AND pre-truncates each cell with `first_line`, so what does not fit ends in "…"
   rather than reading as a complete, different host.
5. ~~**Maintenance renders per-row fetch failures as ordinary body text**~~ — CONFIRMED
   and fixed in v0.49.1. The escape codes showed it exactly: the label was `38;5;8` and the
   error value was `[39m`, the terminal's DEFAULT foreground — identical ink to a real
   Docker version above it. Rows now carry `Result<String, String>` and a failure is drawn
   bold in the error colour, wrapped with a hanging indent so the reason stays readable in
   full.

**The round-2 critique list is now empty.** The next run should start a fresh critique
pass — these screens have changed a great deal since round 2 was written.

### Scalability — the tool must not fold at real scale

The owner runs hosts with hundreds of services and 700+ domains. Things to watch and
improve, each verifiable:

- ~~**Render cost** on huge tables~~ — MEASURED and fixed in v0.48.5. A `#[ignore]`d
  benchmark lives in `tui/tests.rs` (`bench_render_cost`); run it with
  `cargo test bench_render_cost -- --ignored --nocapture`. Keep using it rather than
  guessing — it corrected me twice in one run.

  | services | Services before | after | Monitor before | after |
  |---|---|---|---|---|
  | 50 | 3.19 ms | 2.25 | 2.44 | 1.97 |
  | 200 | 17.7 ms | 4.06 | 6.72 | 4.27 |
  | 500 | 89.7 ms | **7.94** | 14.98 | 9.17 |

  The cost was `metric_for` — a linear scan over the metrics list, called two or three
  times per row, i.e. O(services²) on a path that runs every frame. `App::metric_index`
  and `App::deploying_index` build the lookups once per frame instead.

  Two hypotheses were measured and DISPROVEN first, both plausible from reading the code:
  (a) the per-frame `app.monitor.clone()` in the Monitor render — real, but only worth
  ~6 ms at 500; (b) `visible_rows()` rescanning all services per project (O(P×N)) —
  fixing it moved 89.7 → 87.2 ms, i.e. nothing. Do not skip the measurement step here.

  Note the live host has 48 services and 46 domains, NOT the "hundreds / 700+" this
  section assumed — that scale can only be reached synthetically, via the benchmark.
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

**A faithful copy can still be a broken one** (v0.51.1). Cloning a MySQL replica
copied its config file exactly — and `super_read_only = ON` stops a FRESH database
from initialising, because the entrypoint has to write the root password, the user
and the schema. The clone came up with none of them while the panel displayed the
credentials it thought it had set, and since a database initialises only once, the
failed boot cannot be repaired by fixing the config afterwards. When copying
configuration onto something EMPTY, ask what that configuration assumes already
exists.

**Walk one workflow END TO END, don't spot-check.** Five rounds of screen-by-screen
critique had gone dry. Driving a single create-service run start to finish — pick a
project, name it, choose each source type, submit, inspect the result — found two
defects in one screen within minutes (v0.50.5): a Build step offered for a prebuilt
image, and a refusal left pinned to the border after the field it named was gone.
Critique looks at screens in isolation; a walk exercises the *transitions between*
them, which is where state goes stale.

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

## Code health — audit and refactor to stop the codebase bloating (standing)

A run may spend itself paying down code weight instead of shipping a feature — and
*should*, when the code is visibly bloating. This is real work, not filler. It is held to
one hard bar: **behaviour-preserving, and proven so.**

**What counts as bloat here — hunt these:**

- **Files that outgrew one sitting.** `app.rs`, `render.rs`, `keys.rs` are the largest.
  When a file passes ~1,000 lines or mixes several concerns, split it along the boundaries
  the project already set — `worker` (I/O), `app` (state + selectors), `keys` (key/mouse →
  action), `form`/`table` (shared vocabulary), `render` (draws, decides nothing) — the way
  the 5,000-line `src/tui.rs` was split into `tui/`.
- **Duplicated flows.** The per-service edit forms (Source / Build / Resource) and the
  `fetch → form → save` round-trips share one shape; the `Req`/`Resp` match arms rhyme. If
  a *fourth* copy of a shape appears, extract the shape — don't paste it a fourth time.
- **Dead weight.** Unused fields/functions, stale `#[allow(...)]`, commented-out blocks,
  a `View`/`FormKind` variant nothing constructs.
- **Over-long functions and deep nesting** a small helper would flatten.
- **Over-abstraction is also bloat.** An interface with one impl, a config for a value that
  never changes, a generic used once. Remove these; never add them (ponytail: the best code
  is the code never written).

**How — non-negotiable:**

- **Prove it changed nothing.** The test suite stays green *and* the list of test names is
  identical before and after (the `tui/` split used exactly this as its proof: same 83
  tests, diffed identical). If a refactor forces a test to change, it is not a pure refactor
  — say so, and treat it as behaviour change.
- `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`, `cargo test` all clean.
- **One coherent refactor per run** — a diff a reviewer can follow in a single pass, not a
  scattershot sweep across the tree.
- **Never mix a refactor with a feature in the same commit.** The "nothing else moved" proof
  only holds if nothing else moved.
- **No behaviour change → no release, no version bump, no tag.** Just commit it, with a
  message stating what moved and why. This is the *one* kind of run that ships without a
  release — it does not contradict "every change ships a release" below, because there is no
  user-visible change to release. Still update `CHANGELOG.md`? No — a pure internal refactor
  has no Keep-a-Changelog entry; leave the changelog alone.

**Do not refactor for its own sake.** Clean code that isn't bloating is *done*; touching it
to "improve" it manufactures the very churn this guards against and risks a regression a
behaviour-neutral diff was supposed to avoid. Refactor when there is measurable weight to
remove, and leave it alone otherwise. A run that audits, finds the code healthy, and changes
nothing is a success — report the audit and stop.

## Feature consistency & gaps — hunt asymmetries (standing)

A run may spend itself finding and closing places where the tool is *inconsistent with
itself* — a capability offered in one place but not its sibling. These are often the highest
"feels finished" wins, and several real ones were found exactly this way (ports could be
*added* but not *deleted* until v0.21.0; click-to-select worked only on Services until
v0.23.0). Look at the tool the way a user does: "I can do X here, why not there?"

**Asymmetries to hunt — each is a real pattern this repo has hit:**

- **CLI vs TUI parity.** A capability exists in the CLI but not the TUI, or vice versa.
  (The CLI has `mount-add`; the TUI's Mounts view is still read-only — a live gap.)
- **Add/edit without delete, or list without act.** If you can create a thing, you should be
  able to remove it from the same surface (ports were add-only). If a view lists rows, ask
  whether the obvious action on a row exists.
- **Sibling screens that drifted.** Two screens with the same shape but different
  affordances for no reason — the same key doing different things, one confirming a
  destructive action while the other doesn't, one clickable while the other isn't. Domains
  has full create/edit/delete/primary; compare every other per-service resource against it.
- **Partially-covered endpoint groups.** A `backend.js` group where the tool uses some
  operations but not the natural companions (delete present, update absent; `getX` used but
  the paired `setX` never wired).
- **Inconsistent language for one concept.** The same state labelled differently in two
  places, a status phrased one way in the table and another in the viewer. Pick one.
- **Wizard vs edit drift.** A field editable after creation but not offerable at creation
  (or the reverse) when the API accepts it inline — the create wizard's still-open steps
  (ports, resources) are exactly this.

**How to act on a finding:**

- If it is implementable and live-verifiable now, **close it** — one per run, held to the
  same Definition of done as any feature (verify against the server with a `zzz-*` target,
  ship a release).
- If it needs the live server or a shape you can't confirm, **write it into the backlog
  above** (or open an issue) with the exact asymmetry, and stop — do not guess.
- A pure inconsistency in *labels/affordances* with no API involved is still worth fixing,
  and is often a small, safe diff.

Report the asymmetries you found even when you only close one — the list is the map for the
next run. And do not manufacture asymmetry: two things that are *meant* to differ (a
read-only history screen vs. an actionable table) are not a gap.

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
