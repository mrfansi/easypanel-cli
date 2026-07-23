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

### PRODUCTION HOST — read-only (owner, 2026-07-20)

**`viding-idc` (idc.viding.org) is production.** It is also the biggest host the owner
runs — ~713 domains, ~100 services — which makes it the best place to FIND bugs and the
worst place to make them.

**Browse it. Never mutate it.** No `zzz-*` targets, no create, no deploy, no destroy, no
domain or env write, no backup run. Reading — tabs, filters, arrows, viewers, the Hosts
table — is fine and is exactly how the v0.64.1 filter bug was found.

The trap: it is often ALSO the **default** server, so anything that verifies against the
live API without naming a host lands on production.

**Checking once per run is NOT a guard.** On 2026-07-20 a run checked the default at the
start (`harisenin-angelia`, safe), and by the time it was driving the TUI the file had
changed and the session was on `viding-idc` — the owner switches hosts while the agent
works. Two rules follow:

- The authoritative answer to "which machine am I on" is the **running TUI's title bar**,
  not a config read from earlier. Capture it.
- Re-check **immediately before** each mutation, never once at the top of the run.

Before any mutation, check which host is default:

```
jq -r '.[]|select(.default)|.name' ~/.config/easypanel/servers.json
```

and pass `--server harisenin-aurel` (or `harisenin-angelia`) explicitly for every `zzz-*`
verification. Do not switch the default to make this easier — the owner's choice of
default is theirs, and changing it is itself a mutation of their config.

### Wildcard domains rendered identical to their apex — fixed v0.76.0 (2026-07-21)

The Domains screen showed `https://edu.harisenin.net/` on TWO rows; they looked like a
duplicate. The owner's panel screenshot showed the truth: one was `edu.harisenin.net`, the
other `*.edu.harisenin.net`. EasyPanel stores a wildcard as a SEPARATE flag —
`{ host: "edu.harisenin.net", wildcard: true }`, host string unchanged (verified live via
`project inspect --json`). `domain_source` never read the flag, so wildcard and apex
rendered byte-identical on the one screen you open to tell domains apart. Fixed in
`domain_source` (host → `*.{host}` when `wildcard`), which flows to the TUI, CLI
`domain list`, cross-host domain diff, and `project export` at once. One test.

**Two lessons worth keeping:**

- **A "duplicate" in a rendered list can be the RENDERER lying, not the data.** This run
  started down a "detect duplicate domains" feature — same source+path = duplicate — and
  had the function + tests written before the screenshot revealed the two rows were never
  the same domain. A dedup built on `host+path` would have IGNORED the same wildcard flag
  and confidently flagged a false positive. When two rows look identical, first ask whether
  the tool is hiding a field that distinguishes them. Verify the raw API shape before
  building anything that treats rows as equal.
- **Measure-before-build killed the feature honestly.** After the fix, the live host has
  ZERO true duplicates, so a duplicate-detector had no instance to justify it — that is the
  "don't invent filler" rule doing its job. The dropped code shape (a `duplicates()` in
  domains.rs returning ids + a conflicting count, marked `≡` amber on the source column) is
  in this session's history if a real duplicate is ever measured on a host.

### IMPLEMENTATION RULE (owner, 2026-07-22): always DDD

Standing rule for **every** change, features included — not just refactors. Follow
Domain-Driven Design: a pure **domain model** (types + functions, no I/O) separate from
the **application layer** (orchestration / `commands`) and **infrastructure** (client/HTTP,
TUI worker/render/keys). Domain decisions and rules live in the domain module for their
bounded context (`cloudflare.rs`, `backup.rs`, `dump.rs`, `uptime.rs`, `domains.rs`, …),
NOT scattered across the worker/render/keys. When adding code, put the rule/decision in
the domain layer and keep worker/render thin (a caller + presentation). New Cloudflare
products (R2, D1, …) each get their domain types + pure functions in `cloudflare.rs` (or a
sibling), with the client/TUI as thin consumers.

**The EXISTING architecture should move toward this too (owner, 2026-07-22): the whole
repo should read as DDD, not just new code.** So a valid directive-1 refactor run is:
pick one place where domain logic currently lives in `tui/worker.rs` / `tui/render.rs` /
`tui/keys.rs` (or is duplicated between the CLI and the TUI worker) and PULL it into its
domain module (or a shared `commands::` orchestration), leaving the worker/render as a
thin caller — behaviour-preserving, test names identical, no version bump. Do these one
coherent extraction at a time (the `*Ui` struct extractions and the shared
`commands::dump_to_r2`/`restore_from_r2` are the model). Don't rewrite everything at once.

### FOCUS (owner, 2026-07-22): Cloudflare TUI — mirror EasyPanel behaviour

Standing loop focus, owner-requested: **keep improving the Cloudflare TUI so it
behaves exactly like the EasyPanel TUI.** The two workspaces (`W` toggles) must feel
like one app. Concretely, replicate EasyPanel's patterns in the CF workspace:

- **Header = "Cloudflare — <account>"** (like "EasyPanel — <server>"); the second line
  is a **product tab bar** (DNS today; D1/R2/KV/Workers/Connectors later) styled like
  EasyPanel's tab bar; per-screen **key hints live in the status bar**, not the header.
- **Account switching is a PICKER (`a`), the analogue of the `s` server switcher — NOT a
  tab.** Do not turn accounts into tabs (owner was explicit, twice).
- **Records is a drill-in** from a zone (Enter), like EasyPanel's Logs/Terminal/Credentials
  are drill-ins — not a tab.
- Match EasyPanel's affordances inside the CF screens: `/` filter with the count in the
  title, `v`/`V` mark + `Space` group menu for bulk, `r` refresh, confirmation dialogs
  for destructive actions, the busy spinner, empty-vs-loading-vs-error states, mouse
  clicks where EasyPanel has them. When something works one way in EasyPanel, make CF
  work the same way.
- **Verify by DRIVING it** (owner has 2 live CF accounts configured: `mrfansi-dev`,
  `pt-karya-kaya-bahagia` — the latter's token has DNS perms and real zones/records, so
  the read happy-path is verifiable live; `mrfansi-dev`'s token lacks the Zone:DNS perm,
  which is a good error-path fixture, NOT a bug). LOOK at the screen before calling it done.
- **Web Analytics belongs inline on Domains** (DONE v0.93.3): Cloudflare's
  `GET /accounts/{account_id}/rum/site_info/list` data enriches the Domains/zones table
  with Web Analytics status/setup/date columns. Keep this as listing metadata, not a
  separate product tab or drill-in screen. Metadata auth failures are non-fatal (v0.93.4):
  zones remain visible and the status hints at **Account Settings Read**.
- Known follow-up: when a DNS op returns Cloudflare's "Authentication error", add a hint
  that the token likely lacks the **Zone : DNS** permission (use the "Edit zone DNS"
  template) — a common pitfall, proven via curl this session.

### BACKLOG (owner-requested 2026-07-22): more Cloudflare products — R2, D1

The CF workspace's product tab bar was built to grow (`CF_PRODUCTS`; DNS is tab 1).
Owner wants these next products, each a NEW product tab beside DNS. Each is a real
vertical slice — brainstorm a spec first (docs/superpowers/specs/), verify live READ-ONLY
against the owner's `pt-karya-kaya-bahagia` account (has real resources; never mutate
production — use owner-supplied throwaway names for any create/delete test). Account-scoped:
all need the account's `account_id` (already stored on `CloudflareAccount`), and the API
token must carry the matching permission (surface a clear "token lacks R2/D1 permission"
hint on the auth error, same pitfall as Zone:DNS).

- **R2 (object storage) — buckets DONE v0.84.0, object browsing DONE v0.85.0.**
  `CfProduct::R2` tab. Buckets: `GET/POST/DELETE /accounts/{account_id}/r2/buckets`
  (list result nests as `result.buckets`; cursor pagination). Objects: `GET /accounts/
  {account_id}/r2/buckets/{bucket}/objects` (list `result` a BARE array; `result_info.
  cursor`+`is_truncated`). Domain `R2Bucket`/`R2Object` + `list_r2_buckets`/`list_r2_objects`
  on `CloudflareClient`; TUI Buckets→Enter→Objects drill-in; CLI `cf r2 bucket …` /
  `cf r2 object list`. Verified live read-only.
  **CORRECTION to the old note above — object ops DO NOT use the S3 API / s3.rs.** The
  earlier assumption (objects are S3-only, store R2 S3 access-key/secret, reuse s3.rs) was
  WRONG: Cloudflare's REST API lists objects with the SAME Bearer API token as buckets —
  no separate R2 S3 credentials at all. `s3.rs` stays DB-dump-only. The needed token
  permission is account-scoped **Workers R2 Storage** (Read for list, Edit for
  create/delete); the `r2_hint` surfaces that on an auth error.
  **object upload / download / delete + bulk — DONE v0.89.0.** REST
  (`PUT/GET/DELETE /accounts/{account_id}/r2/buckets/{bucket}/objects/{key}`, Bearer,
  NOT S3). Domain in `cloudflare.rs` (`encode_object_key`/`object_basename`/`upload_key`/
  `MAX_REST_OBJECT_BYTES=300MB` + `put_object`/`download_object`/`delete_object`); CLI
  `cf r2 object put/get/rm`; TUI `u` upload, `Enter`/menu download, `x` delete (confirm),
  `v`/`V` mark + `Space` bulk (Download/Delete N). 300 MB single-PUT cap enforced (bigger
  needs S3 multipart, out of scope); downloads stream; keys percent-encoded (slashes
  literal). Verified live end-to-end on a throwaway bucket (upload→download byte-identical
  →bulk-delete→cleanup). Token needs account-level Workers R2 Storage **Edit** for writes.
- **D1 (serverless SQL database).** New `CfProduct::D1` tab. Databases via
  `/accounts/{account_id}/d1/database` (list/create/delete); run SQL via
  `POST /accounts/{account_id}/d1/database/{database_id}/query` (or `/raw`), which returns
  result rows + meta. TUI shape: Databases list (like Zones) → Enter → a query/tables view
  (list tables, run a read-only `SELECT`, show rows in a table; guard destructive SQL
  behind a confirm). Uses the API token (Bearer), account-scoped. Verify the query
  request/response shape against the official D1 API docs before building.

Both are their own dedicated runs (client + domain + CLI + a new product tab), not hourly
nibbles. Keep the DNS behaviour untouched; each product is an independent arm under the
existing CF workspace + tab bar.

### DONE v0.83.0 (2026-07-22): Cloudflare — zones + DNS records (CLI + isolated TUI), owner-requested

A whole new bounded context OUTSIDE EasyPanel, owner-requested: manage one or more
Cloudflare accounts' zones and DNS records. `src/cloudflare.rs` (domain types + pure
fns + `CloudflareClient`), `CloudflareConfig` store (`cloudflare.json`, standalone,
multi-account — NOT tied to a server), CLI `easypanel cf account/zone/record …`
(incl. bulk `cf record set --where-content OLD --content NEW` to repoint many records
in one command), and an ISOLATED TUI workspace (key `W`, orange): Zones home for the
active account, `a` account picker (mirrors the server `s` picker — select/add/delete;
adding the first account auto-activates it), Enter-on-zone → Records drill-in with
add/edit/delete + bulk (v/V mark + Space menu) + `/` filter. Record edits use Cloudflare
**PATCH** (partial), filter uses the `name.contains=`/`content.contains=` operator keys.
Design + plan under `docs/superpowers/specs|plans/2026-07-22-cloudflare-*`.

**NOT LIVE-VERIFIED (the one gap) — do this on first real use with a token:** the tool
has no Cloudflare token in-session, so only the request/ERROR plumbing is proven live (an
invalid token returns Cloudflare's real error envelope, surfaced in the TUI error state).
The full create/read/update HAPPY-PATH and six scoped-token edge cases are unconfirmed —
see the "API grounding" appendix in the spec for the exact list (max `per_page`; whether a
zone-scoped token can `GET /zones` without `account.id`; whether `account.id` is truly
optional at `POST /zones`; the PATCH "only changed fields" wording; the zone `name` filter
operator syntax). When a token is available: add an account, list zones, add/edit(bulk)/
delete a record on a THROWAWAY zone, and fix any shape that the docs got wrong. All pure
logic is unit-tested (envelope, record_body, apply_patch, resolve_zone, select_records,
filter_query); no wrong-shape HTTP mocks were written (per the "a wrong mock is worse than
no test" rule).

### DONE v0.82.0 (2026-07-22): multi-DB dump hang FIXED + restore-from-R2 in the TUI

**The big one — multi-database dumps hung.** An operator hit "Dump did not report
completion within 10 min" dumping 4 production databases. Root cause (traced live,
NOT guessed): `container::run_until_done` passed the whole command inside the
WebSocket connection URL (`ws_url` → `?command=<base64>`). A multi-DB command
(several schema names + the ~380-char presigned URL) overran the URL, arrived
TRUNCATED, and the shell hung on an unterminated line → no marker → full-cap wait.
Single-DB commands were short enough to fit. Confirmed by running the identical
full command as terminal INPUT (worked, 33 s) vs via the tool (hung). FIX: send the
command as WebSocket **input** to a plain `sh` (input has no length limit — the
interactive terminal already proves it), and detect the completion marker by its
RESOLVED digits (`__EZP_DONE_<code>__`) so the PTY-echoed `printf '…%s…'` isn't
mistaken for it. Verified live: the 4-DB ~100 MB dump now finishes in ~35 s. NOTE:
`run_once` still uses the URL command (its commands are short — SHOW DATABASES etc.
— so fine); if a future short command ever grows, move it to input too.

**Also ruled out along the way (keep, saves a future run the dig):** not disk
(`/tmp` had 581 GB), not the dump (4 DBs → 545 MB file in 12 s), not gzip (18 s),
not an R2 single-PUT size limit (a 130 MB single PUT succeeded), not a WS idle
timeout (a 45 s-silent command survived). It was purely the command-in-URL length.

**Restore-from-R2 in the TUI (the other half of v0.81.0's dump).** Storage ▸ now has
"Restore from an object-storage dump": `s3::sign_list` signs an S3 ListObjectsV2 for
the `{project}/{service}-` prefix (EasyPanel lists no such files), `commands::
list_r2_dumps`/`restore_from_r2` are shared with the CLI (`db list`, `db restore`),
worker `Req::R2Dumps`/`RestoreR2`, picker via `BackupUi.r2_restore_into`. Verified
live: the picker listed a dump and restored it. mysql/mariadb only.

Minor nit — RESOLVED (2026-07-22, [Unreleased]). The note claimed the restore's
success status was overwritten by the `Refresh::Projects` reload showing "Ready".
That half was STALE: `Resp::AllServices` never touches `status`, so "Restored …"
already survived. But the real half held — a restore imports rows INTO a database
and changes nothing in the service table, so `Refresh::Projects` was a wasted full
reload. Switched `RestoreR2` to `Refresh::None`, matching `DumpR2`. Too small to tag;
recorded under [Unreleased], rides the next release. Lesson: a "shows Ready" symptom
in a note is worth re-deriving from the code before trusting it — the overwrite path
it named didn't exist.

### DONE v0.81.0 (2026-07-22): the non-locking dump is now in the TUI too

v0.80.0 shipped `db dump`/`db restore` CLI-only; the TUI still offered EasyPanel's
locking native backup (owner noticed the gap). Closed: the dump/restore
orchestration is now a shared `commands::dump_to_r2` (used by both the CLI and a new
`worker::Req::DumpR2`), and a mysql/mariadb service's **Storage ▸** menu has "Dump
now (non-locking) → object storage" above "Backup now". Reuses the existing database
picker via a `BackupUi.r2_mode` flag (confirm action `"r2dump"`). Verified live from
the TUI: dumped two seeded databases into one R2 file with the row intact.

**TOP NEXT ITEM — restore an R2 dump from the TUI (close the other half).** The TUI
can now DUMP to R2 but can only RESTORE via the CLI, which is the asymmetry to
finish. It needs: (a) list the tool's own dumps — there's no EasyPanel endpoint, so
sign an S3 `ListObjectsV2` for prefix `{project}/{service}-` (extend `s3::presign`
to take extra query params, or header-sign; fetch with reqwest, regex the `<Key>`s);
(b) a picker → confirm → a new `worker::Req::RestoreR2` calling a shared
`commands::restore_from_r2` (extract it from `db_restore` the same way `dump_to_r2`
was extracted); (c) a "Restore from an object-storage dump" menu entry. Mirror the
dump side exactly. Mysql/mariadb only (curl).

### DONE v0.80.0 (2026-07-21): a non-locking DB dump straight to R2 — SHIPPED

**Built and verified live end-to-end.** `easypanel db dump` / `db restore`
(mysql/mariadb): `src/s3.rs` (Sig v4 presigner, tested vs AWS's published vectors),
`src/dump.rs` (dump/restore command builders + shell-injection gate on db names),
`src/container.rs::run_until_done` (long-running one-shot exec with a `__EZP_DONE_`
marker for the exit code), CLI in `commands.rs`/`main.rs`. Proven on throwaway
zzz services: seeded a row → `db dump` to R2 (non-locking, `--single-transaction`)
→ `db restore` into a DIFFERENT service that never held the DB → the table AND its
row arrived. Cleaned up all zzz services + R2 objects.

**Hard-won gotchas (keep for postgres/mongo extension):**
- The container shell is a PTY (`docker exec -it`). Do NOT pipe a real-sized dump
  through it (`mysqldump | gzip`) — the pipe write hits `errno 11 (EAGAIN)` and
  corrupts/aborts. Write mysqldump to a FILE, then gzip the file, then upload.
- The command travels in the containerShell URL; keep it SHORT and FLAT. A braced
  / subshell wrapper (`{ …; } … ( exit $? )`) was truncated to "syntax error:
  unexpected end of file". No `{ } ( ) [ ]` grouping in the command.
- `--set-gtid-purged=OFF` (mysql) both silences the GTID warning (no stray stderr
  to the PTY) and makes the dump restore without GTID conflicts.
- R2 rejects a streamed/chunked PUT with `411`; buffer to a file so Content-Length
  is set, then `curl -T file`. `getServiceDatabases` returns a plain array incl.
  system schemas; `--all` filters those out (dump::is_system_db).
- A leftover idle mysql connection holding a lock on the target DB will hang
  `mysqldump --single-transaction` (bit us via a stray DB-shell session). Normal
  operation is unaffected; noted in case a future run sees a mysterious hang.

**postgres — ATTEMPTED 2026-07-21, BLOCKED by the image (do NOT retry as-is).**
The command builders are easy (`pg_dump -U <user> --create --clean --if-exists
<db>` per database → file; restore `psql -U <user> -d postgres -f file`; creds are
`/user`+`/password`, non-locking is free). The blocker is the TRANSPORT: the
**official `postgres` image ships NO `curl` and NO `wget`** (verified live on
`postgres:17` — `command -v curl wget` is empty). The whole architecture uploads
container→R2 with `curl` from inside the container, and postgres has no HTTP client
to do it, so a postgres `db dump` fails at the upload (or hangs). mysql/mariadb
images DO have curl, which is why v0.80.0 works for them. v0.80.0 already rejects a
postgres service cleanly at `resolve_db_engine` ("supports mysql and mariadb"), so
there is no bug to fix — postgres simply can't use this path.
  To actually ship postgres you must change the TRANSPORT so it doesn't need an
  in-container HTTP client. Best option: dump to the container `/tmp`, then read the
  gzipped file back over the WebSocket **base64-encoded** (text survives the PTY;
  raw binary EAGAINs — see [[container-exec-pty-constraints]]), and upload from the
  TOOL with reqwest (already a dependency) + the `s3.rs` presigner. That also drops
  the curl dependency for mysql/mariadb. Caveat: the dump then crosses the WS, so
  test whether the containerShell endpoint has the same ~125 s proxy limit that
  killed the original "stream to laptop" idea (it may not — it's a persistent
  socket, not a single blocked request). This is a real design change, a full
  focused run, not a nibble.

**Also not yet done:** mongo (`mongodump --archive --gzip`, needs the same
transport rethink + it's not one SQL file); a pre-flight free-disk check on the
container `/tmp`; and `run_until_done` does not kill the in-container command when
it gives up (a blocked dump leaks a connection) — harmless normally, worth a
follow-up.

--- original scoping notes (kept for reference) ---

### BACKLOG (owner-requested 2026-07-21): a non-locking DB dump straight to R2 — FEASIBILITY PROVEN, build it

The owner's real pain with EasyPanel's native backup, in their words:
1. It **locks/hangs the running DB** during backup (no `--single-transaction`), so
   apps using that DB error out.
2. It's **per-database, one file each**, and restore **requires the target database
   to already exist** — so restoring to a NEW server (where the DB has never existed)
   fails with `[400] … docker exec … mysql … exit code 1` (Unknown database).
3. Owner wants a backup that is non-locking, one self-contained file for multiple
   DBs, AND still stored in the EXISTING R2 storage provider (not just the laptop).

**The solution (owner's own correct commands):** run our OWN dump in the container
via the WebSocket shell, non-locking, and upload it to R2 with a presigned URL.
```
# mysql
mysqldump -u$U -p$P --databases $DBS --single-transaction --quick --skip-lock-tables \
  --routines --triggers --events | gzip > /tmp/d.sql.gz && curl -T /tmp/d.sql.gz '<presigned-PUT>' && rm /tmp/d.sql.gz
# mariadb: mariadb-dump … --single-transaction --quick --routines --triggers --events --hex-blob --default-character-set=utf8mb4
```
`--databases` embeds `CREATE DATABASE`, so the dump restores onto a fresh server
(fixes pain #2). `--single-transaction` = no lock (fixes #1). Data flows
container→R2 DIRECTLY, so it never crosses the WebSocket → **no proxy 125s timeout**
(the objection that killed the "stream to laptop" idea).

**EVERY link verified live on zzz-* (2026-07-21), then cleaned up — do NOT re-probe:**
- `storageProviders/common/list` (group `storageProviders/common`, op `list`) returns
  the S3 provider WITH creds: `accessKeyId`, `secretAccessKey`, `bucket`
  (harisenin-db), `endpoint` (…r2.cloudflarestorage.com), `region` "auto",
  `subtype` "cloudflare-r2", plus the `id` used by backup meta.
- mysql:9 container HAS `curl` (7.76.1). `mysqldump --single-transaction | gzip`
  works (verified 881KB for 2 DBs earlier).
- **R2 REJECTS a streaming PUT** (`curl -T -`, chunked) with `411 MissingContentLength`.
  MUST buffer the dump to a file first (`… | gzip > /tmp/f`), then `curl -T /tmp/f`
  so Content-Length is set. That worked: **PUT 200**, GET-back matched, DELETE 204.
- Presigned URL recipe that worked: AWS Sig v4, `service=s3`, `region=auto`,
  `X-Amz-SignedHeaders=host`, payload hash literal `UNSIGNED-PAYLOAD`, path-style
  `{endpoint}/{bucket}/{key}`. (Reference Python presigner is in this session's
  history — port it to Rust.)

**Step 1 DONE (2026-07-21):** the container command-exec primitives are extracted
to a top-level `src/container.rs` (pub(crate): `ws_url`, `base64`, `connect_failure`,
`run_once`, `set_read_timeout`) — reachable from `commands.rs` now, not just the TUI.
Pure refactor, 287 test names identical, verified live. So the feature can build on
`crate::container::*` directly; no visibility/architecture work left.

**Build scope for the loop:**
- Deps: add `hmac` + `sha2` (small, pure-Rust) for Sig v4 — the rustls `ring` in-tree
  isn't cleanly reachable. Presigner is a PURE function → test against AWS's public
  Sig v4 test vectors.
- New module (e.g. `src/s3.rs`): presign PUT/GET/DELETE. `src/dump.rs`: per-engine
  dump command builder (mysql/mariadb first; postgres `pg_dump`/`pg_dumpall` and
  mongo `mongodump --archive --gzip` later — mongo is NOT one SQL file).
- Fetch creds via `storageProviders/common/list`; pick the s3/remote provider.
- Container exec: `terminal::run_once` caps at 20s — TOO SHORT for a real dump+upload.
  Need a variant that waits for completion (append `echo __DONE_<rand>__` and read
  until the marker, or until socket close) with a long cap. The WebSocket container-
  exec code (`ws_url`, `base64`, `run_once`) is `pub(super)` in tui/terminal.rs — to
  use it from a CLI command (`commands.rs`) either widen visibility to `pub(crate)`
  or (cleaner, DDD) extract those primitives to a shared `src/container.rs` FIRST as
  a separate pure-refactor commit, then build the feature on it.
- CLI: `easypanel db dump <project> <service> [--databases a,b,c | --all] [--provider <name>]`
  → writes `<project>/<ts>.sql.gz` (or a chosen key) to R2, prints the path. Plus
  `easypanel db restore <project> <service> --path <r2-key>` → presigned GET inside
  the container → `curl … | gunzip | mysql …` (also non-locking import; `--databases`
  dump auto-creates the DB, fixing the cross-server case). `--all` uses
  `getServiceDatabases` (op exists) minus the system DBs (information_schema, mysql,
  performance_schema, sys, system).
- **Caveats to surface to the user:** (a) buffers to container `/tmp` — needs free
  disk ≈ compressed size; warn/guard. (b) This is the TOOL's backup — it will NOT
  appear in EasyPanel's own restore UI (that list comes from backup ACTIONS, which
  we don't write); restore is via the tool's `db restore`. (c) Handles secrets (R2
  key/secret, DB root pw) — never log them; presigned URL carries the accessKeyId
  (not the secret) and is short-lived.
- Verify end-to-end on zzz-*: dump 2+ DBs of a zzz mysql → R2, then restore into a
  DIFFERENT zzz service that has NONE of those DBs, and confirm the data matches,
  before touching anything real.

**Sequencing note (2026-07-21):** this is a WHOLE VERTICAL SLICE, not an hourly
nibble. The crate has no `#[allow(dead_code)]` and clippy runs `-D warnings`, so a
presigner (or `dump.rs`) committed ALONE, with no caller, fails the build — landing
it as an orphan module "for later" is exactly the scaffolding ponytail forbids. So
`s3.rs` must land WIRED to its first real consumer (`easypanel db dump` for mysql)
in the same effort. That effort also needs the long-running container exec (run_once
caps at 20s) AND a live zzz mysql, whose create is a Swarm deploy that blocks for
minutes. Net: give this a DEDICATED focused session, not a "find a quick win" run —
don't defer it into a half-built orphan. `getServiceDatabases` (used by the v0.79.3
restore pre-flight) returns a plain array incl. system schemas and is the `--all`
source (minus INTERNAL).

### Restore into a fresh server fails silently — smaller companion fix (2026-07-21) — DONE v0.79.3
**DONE (2026-07-21, v0.79.3).** The PRE-FLIGHT check shipped. Both restore paths
(TUI worker `Req::RestoreBackup` and CLI `backup db-restore`) now call
`databaseBackups/getServiceDatabases` (returns a plain array of db names,
system schemas included — no stype/password/container needed) and refuse up front
with `crate::backup::missing_database_message` when the target db is not in a
NON-EMPTY list. Key nuance learned live: an EMPTY list means the engine couldn't
be read (a running mysql/postgres always lists its own system schemas), NOT "no
databases" — so `service_lists_database` returns `None` (don't block) for empty
or non-array, `Some(false)` only for a populated list that lacks the db. Verified
live on angelia: `harisenin-com-db/mysql` (system DBs only) → blocked with the
clear message; `edukasistudio-db/mysql-r1` (has `edu_db`) → passes pre-flight.
Pure helpers unit-tested. NOT done (deliberately deferred, low value now): the
optional "offer to CREATE DATABASE then restore" one-step flow — the plain
message is enough and lower-risk; revisit only if operators ask for the auto-create.

Original note: Separate, smaller item (do even if the R2-dump feature isn't
built): EasyPanel's restore needs the target DB to exist. When it doesn't, the
tool showed the raw, truncated `[400] … docker exec … exit code 1`.

### Backup/restore clarity — restore now names the database; bulk backup already exists (2026-07-21)

Owner confusion, from the cross-host restore screen: "I can't tell what databases
are in each backup, and why is multi-database backup one-by-one?" Ground truth of
the EasyPanel backup model (already in backup.rs docs): a database SERVICE holds
several DATABASES (schemas); backups are PER-DATABASE (`createDatabaseBackup`/
`restoreDatabaseBackup` take `databaseName`); the action `meta` carries
`databaseName` + `path`.

- **Fixed v0.79.2:** the "restore from another server" list (`history_all`, worker
  ~L1026 + header app.rs ~L977) showed When/From(project/service)/File but DROPPED
  the database — so every backup of one service read identically. Added a Database
  column. The single-service restore (`BackupFile::row()`) already showed it.
- **Bulk BACKUP already exists — tell the owner, don't rebuild:** "Backup now" opens
  a database picker whose row 0 is "All N databases" (backup_ui.rs:66), enumerated
  via a `SHOW DATABASES`-style listing (`backup::parse_databases`). Tick individual
  databases or pick All. So the multiple backup rows ARE the result of one "All"
  backup, one file per schema.
- **Bulk RESTORE is still one-at-a-time** (one `pending_restore`). A real follow-up
  feature if wanted: multi-select restore (tick several files → restore each). With
  the database names now visible, picking the right ones one-by-one is at least
  unambiguous. Medium; heavier blast radius (writes), verify on zzz-* first.

### App god-struct extractions — TermUi done (2026-07-21), next candidates ranked

The App struct (~70 fields) is being shrunk one cohesive UI-state cluster at a
time — the proven mechanical pattern: move fields into a `*Ui` struct in the
sub-view's home module, `#[derive(Default)]`, rename access sites, ONE test body
changes, test-NAMES byte-identical. Done so far: `ViewerUi` (viewer.rs), `BackupUi`
(backup_ui.rs), `CredsUi` (app.rs), and now **`TermUi`** (terminal.rs — term_parser/
term_input/term_title → `app.term`, no version bump, 286 test names identical).

**Next pure-refactor candidates (architect-ranked, for a future directive-1 run):**
- `UptimeUi` — `watch`/`probes`/`uptime_state`/`checking` into uptime.rs (DDD-aligned).
  Trap: `watch.len()` is read inside status-string formatting, and `watch_action` is
  a PENDING-event_loop-action field (different category) that must be EXCLUDED. Touches
  keys.rs too. Medium risk.
- `ActionsUi` — `actions`/`actions_state`/`actions_failures_only`. Small, but
  `visible_actions()` filtering is entangled; keep it behaviour-identical.
- Do NOT extract anim/mouse (read all over render, 41 sites) or monitor (reaches
  into worker) — footprint too broad to stay clean.

**The trap that makes a `*Ui` extraction NOT pure:** a partial reset. If the old code
nulls only SOME of the cluster's fields (TermUi's switch_server/TermClosed null
parser+input but not title, which is then read), a "tidy" `Struct::default()` reset
wipes the rest and changes behaviour. Keep resets field-by-field; grep the reset
sites before trusting the diff.

### first_line ate meaningful indentation — fixed v0.79.0 (2026-07-21)

Owner (on a 118-service host) reported the Monitor table read as a FLAT list — no
project→service hierarchy. Root cause was NOT the Monitor code: `output::first_line`
(the shared cell truncator that `render_table` runs on every flexible column) did a
full `.trim()`, eating the two-space indent `monitor_rows` puts on a service. So the
indent was stripped at RENDER time even though the data had it — and the Services
tab kept its indent only because `render_projects` runs first_line on the Source
column, not the name. Fix: `first_line` trims trailing only (leading whitespace is
indentation). Plus project-header rows now bold-cyan on BOTH tables (Monitor: detect
by un-indented name; Services: by the `Line2::Project` type — NEVER by an indent
test, since a marked service is `✓ name` not `  name`). Tests: first_line preserves
leading space; a render test asserts a Monitor service is indented.

**Class:** a shared display helper that `.trim()`s destroys any caller's deliberate
leading whitespace. If a table ever needs an indent/tree, check that the truncator
in its path preserves it. Grep other `.trim()` in the render/output path if a
future alignment looks flattened.

### Empty-vs-failed CLASS — now closed across the screens that mattered (v0.78.1 + v0.78.2)

An adversarial fan-out audit (one code-reviewer agent over all 8 data screens)
ranked the Domains bug's siblings. The verdict, and what shipped:

- **Domains** (v0.78.1), **Services** (v0.78.2), **Dashboard/Stats** (v0.78.2) — the
  three that actively mislead. Services drew a bare table reading as "host has
  nothing"; Dashboard was WORST, fabricating 0.0% gauges (`app.stats.unwrap_or(Null)`)
  that look real. All three now use the per-kind pattern: `Resp::<Kind>Err(String)`
  → `App.<kind>_error: Option<String>`, cleared on the next success, branch the
  render's empty/idle state on it, keep last-good on refresh failure. Live-verified
  by pointing a THROWAWAY server (`server add zzz-unreachable --url http://127.0.0.1:1`)
  at a dead URL, then removing it and restoring the config byte-identical.

- **DO NOT "fix" these — they are correct by construction** (audit confirmed, don't
  re-audit): the service-detail collections (ports/mounts/redirects/env/backups)
  open the viewer ONLY on `Resp::Viewer` (success), so their "No X yet" is
  unreachable on a failed fetch; **Hosts** already renders per-host failure as
  "DOWN — <reason>" via the `HostState::{Loading,Ok,Err}` enum (the gold-standard
  pattern the fields above imitate). Nodes/Monitor/Actions/Storage draw blank tables
  on failure — MEDIUM/LOW, left alone as tidiness-not-defects per the audit rubric.

- **Pattern for a NEW network-backed screen:** either add a per-kind `_error` field
  (small) or use the `HostState` enum (cleaner). Never draw a placeholder/zero that
  can't tell "empty" from "failed".

### Two "the screen lied about its state" bugs — fixed v0.78.1 (2026-07-21)

Both found in one run: one by driving the live host, one by a parallel adversarial
UI critic on the recent screens.

1. **Empty-state vs failed-fetch.** On viding-idc (prod) the panel was 502ing;
   `listDomains` failed and the Domains screen drew "No domains yet — press n to
   add one" on a host with hundreds of domains. `Resp::Err` is generic (load AND
   action failures share it), and a GLOBAL load-error flag would be WRONG here:
   `listDomains` (the 713-row call) is exactly the one that times out alone while
   `listProjectsAndServices` succeeds, and that success would clear a global flag.
   So the fix is per-kind: new `Resp::DomainsErr` → `app.domains_error`, cleared on
   the next successful load, shown by the empty-state. If another data screen shows
   the same lie, repeat per-kind (don't reach for a global flag). Render test in
   tests.rs proves empty-vs-failed.

2. **A sub-screen's Esc swallowed by global guards.** The global Esc handlers
   (clear filter, clear marks) run BEFORE the per-screen dispatch. Credentials is
   opened from the filtered Services list, so its advertised "Esc back" silently
   cleared the filter or DESTROYED the marks and stayed put. Fix: `App::screen_owns_esc()`
   (Viewer | Credentials) exempts those from the filter/marks Esc guards, so Esc
   reaches their handler and returns to the list with filter+marks INTACT. The data
   screens still clear filter→marks on Esc. **Class to remember:** any new
   full-screen sub-view opened from a list must be added to `screen_owns_esc`, or
   its Esc will be eaten by the filter/marks guards. Verified live: one Esc from
   Credentials returned to the filtered+marked list; Esc on Services still cleared.

### Bulk resource limits — shipped v0.78.0 (2026-07-21)

Owner tried to bulk-update resource limits (37 marked services on prod) and there
was no such menu entry — single-service `L` only. Built it: mark (`v`/`V`) → action
menu → "Set resource limits on N marked" → one resource form → applied to each via
its own `services/{stype}/updateResources`. New `FormKind::BulkResourceEdit` (no
target; marked set read at submit via `bulk_targets()`), `Req::BulkResource`,
worker `bulk_resource()` reusing `Resp::BulkDone` for the per-item pass/fail
report. Menu entry gated on `has_resource_limits` so it never opens a no-op form;
compose types are reported by name, not silently skipped. Verified live on a
throwaway (cpuLimit=0.5/memLimit=512 onto 3 app services in one submit, confirmed
via inspectService, torn down). Pattern to reuse for the NEXT bulk-config request
(bulk env? bulk basic-auth?): form → resolve `bulk_targets()` at submit → worker
loop → `BulkDone`. updateResources only stores config; a deploy applies it.

### README screenshots — reproducible pipeline, use demo data (2026-07-21)

Directive-6 README work. There is NO screenshot tool on this box (no freeze,
silicon, imagemagick, cairosvg, rsvg). Since this is a terminal app, a
"screenshot" is the terminal's own output — so `docs/ansi2svg.py` converts a
`tmux capture-pane -e -p` dump into a self-contained coloured SVG (GitHub renders
SVG images but NOT ANSI colour in code fences). Regenerate:
`tmux capture-pane -t <s> -e -p | python3 docs/ansi2svg.py "Title" > out.svg`.
Verify visually with `qlmanage -t -s 1100 -o /tmp/out docs/screenshots/x.svg`
(run it BACKGROUNDED — qlmanage blocks; poll for the .png) then Read the png.

**Owner's rule (asked 2026-07-21): screenshots must use DEMO data, not real
infrastructure.** Real captures expose the owner's production topology (domains,
project names) to a public README. The accepted approach: stand up a throwaway
project on the SAFE host with neutral names (acme-shop, *.shop.example.com),
deploy a couple of `nginx:alpine` apps + a redis + a postgres (they show `active`
with real metrics in seconds), screenshot Services/Domains/Credentials, then
DESTROY the project and verify all three hosts are clean. The server label in the
title bar is the one thing you cannot make "demo" without mutating the user's
config, so `sed` it to a neutral name in the captured text before converting.
Facts learned building the demo: a wildcard domain needs `certificateResolver`
set (400 "Wildcard domains require a certificate resolver" without it); databases
provision on createService and show `active` even though `deployService` on a db
type answers 404. Keep credentials MASKED in any published screenshot.

### DB credentials view & copy — shipped v0.77.0 (2026-07-21)

Owner asked: a database service should show its credentials like the panel's
Credentials screen (user, password, internal host/port, connection URL) and let
you copy the password. Built it: Shell menu (`t`) → Credentials on any
mysql/mariadb/postgres/mongo/redis service opens a read-only `Screen::Credentials`;
`v` reveals/hides, `c`/`y`/Enter copies the selected field. New `credentials.rs`
bounded context turns inspectService fields into the display identity (same fields
the DB shell authenticates with). Verified live on redis + mysql (aurel).

**Clipboard = OSC 52, deliberately (reference for any future copy need).** No
clipboard crate (arboard needs X11/Wayland libs and fails headless) and no
pbcopy/xclip shelling. OSC 52 writes `ESC ] 52 ; c ; <base64> BEL` to stdout;
reused the hand-written `terminal::base64`. It reaches the real clipboard over SSH
and through tmux with `set-clipboard on` — exactly this tool's habitat. Set via
`app.clipboard: Option<String>`; the event_loop (which owns the terminal) emits it.
Caveat: terminals without OSC 52 (Terminal.app) silently no-op — the status line
"X copied to clipboard" is the honest feedback either way.

**Follow-up refactor available (NOT done, would be its own run):** `terminal::db_command`
(the DB shell login builder) and `credentials::credentials` both encode the same
per-type credential facts. They could be unified so the shell command is built from
the credentials context — a pure refactor for a later run, kept out of this feature
commit per the no-mixing rule.

### Repoint an orphan: ALREADY POSSIBLE — do not build a new feature (2026-07-21)

Follow-up to the delete-orphan dead end. Probed live: `updateDomain` DOES accept an orphan
and repoints it to a live service (`{}` success, verified the destination changed), and
once repointed the domain is deletable normally. So the fix for a dead route is to REPOINT
it, not delete it.

But no new feature is needed: the existing `e` edit form already does exactly this. Opened
on an orphan, its Service dropdown lists the project's live services, picking one and
saving repoints it (verified end to end in the TUI, "Domain updated", destination now the
live service). So the operator workflow already exists — v0.69.0 marks the orphan with ✗,
`e` repoints it. Do not build a redundant "repoint" action.

### Bulk-delete orphan domains: BUILT, then BACKED OUT

### Bulk-delete orphan domains: BUILT, then BACKED OUT — the panel won't delete them (2026-07-21)

Probed before shipping, and the probe killed the feature — which is the probe doing its
job. Findings against a live non-production host:

- **`deleteDomain` validates the destination and rejects an orphan.** A domain pointing
  at a service that does not exist is refused with `[400] Wrong service type.` — the exact
  domains a "delete orphans" button would target. Verified: created domains pointing at a
  ghost service, and every deleteDomain returned that 400.
- **Destroying a service CASCADE-deletes its domains.** So the normal path never produces
  an orphan; the domain goes with the service. Real orphans (1 of 713 on the owner's prod
  host) arise from other paths — a rename, a manual mispointing — and the one real orphan
  is structurally identical to the ghost ones, so deleteDomain would almost certainly
  refuse it too. Not tested against the real one: it is production and must not be deleted.
- **`deleteDomain` back-to-back is also unreliable** (some calls in a tight loop error
  transiently while the domain survives) — so IF it ever became deletable, bulk delete
  would still need to be sequential with per-item failure reporting, never a fan-out.

The feature was fully built (preview → confirm → sequential delete with a failure report,
`D` on the Domains screen) and verified to identify/preview orphans correctly and report
failures honestly. But it was backed out unshipped: a destructive button whose targets
the panel refuses to delete is worse than none. The orphan MARKING (v0.69.0) stands — it
tells the operator; deletion is the panel's to fix (or a `viding-static2`-style domain has
to be repointed, not deleted).

**Do not rebuild this** unless a future EasyPanel makes deleteDomain accept an orphan, or
a repoint-then-delete flow is found. If revisited, the code shape is in git stash history
of this session's abandoned work.

### git checkout <file> threw away uncommitted work AGAIN (2026-07-21)

Second time in two runs. While sabotage-testing, a python patch's anchor did not match
(an apostrophe-escaping slip), so nothing was sabotaged — and then `git checkout
src/tui/app.rs` reverted the whole file, deleting an uncommitted helper. The brief already
says use `git stash`; this run proves the note is not enough on its own.

Hard rule for the sabotage step: **never `git checkout <file>` on a file with uncommitted
work.** To try a change and undo it, use `git stash push -- <file>` then `git stash pop`,
or edit the one line back by hand. And check the patch actually applied (grep for a marker)
before running the test — a no-op sabotage that "passes" proves nothing, which is its own
recorded lesson.

### The parked "migrate five tables to flex_width" refactor is NOT a refactor (2026-07-21)

Investigated and dropped, with numbers rather than a feeling:

- Two of the five (Domains, Services) have **two** flexible columns. `flex_width` returns
  `None` for that on purpose — ratatui shares the slack and no single answer exists. They
  cannot migrate at all.
- The other three each compute a number that DIFFERS from the helper. Measured for the
  Uptime table at 100 columns: hand-rolled **42**, helper **44** — the hand-rolled ones
  count the highlight symbol and borders slightly differently and come out conservative.

So unifying them changes what is on screen, which makes it a behaviour change wearing a
refactor's clothes — and the benefit is two more characters of a URL. Not worth a release
and not eligible as a "pure refactor". The real win (one helper, applied automatically
inside `render_table` so a NEW table cannot forget it) already shipped in v0.67.0.

Do not re-park this. If a table is ever rewritten for another reason, let it adopt
`flex_width` then.

### A vacuous test is worse than no test (2026-07-21)

Writing the regression test for the storage-path truncation took THREE attempts, and the
first two passed against the broken code:

1. The terminal was wide enough that the path fitted — the assertion never ran on a cut
   value at all.
2. The assertion looked for `mysql-r1`, which also appears in the **Service column** of
   the same row. The path was being cut and the line still matched.

Both times the test was green and proved nothing. The rule: **a regression test is not
finished until it has been seen to FAIL against the old behaviour.** Disable the fix,
run it, watch it go red, then restore. And when a test asserts on rendered output, assert
on a string that can only appear in the cell under test — a row contains many columns.

(Also: use `git stash`, never `git checkout <file>`, to try a sabotage — a checkout threw
away an uncommitted fix during this very exercise.)

### Open, not fixed: the DB shell password rides in a URL query string

`ws_url` (terminal.rs) puts the base64'd shell command — which for a database shell
contains the root password — in the WebSocket **query string**, along with the API token.
Any proxy in front of the panel (Cloudflare, nginx, Traefik) logs full request URIs by
default, so both land in access logs for the retention period. Base64 is encoding, not
protection.

Not fixable client-side: EasyPanel's `/ws/containerShell` takes both as query params.
A real fix needs the panel to accept the command as a post-connect frame. Recorded rather
than papered over. If a future EasyPanel exposes that, take it.

### Secrets: an exclusion LIST is not a policy (2026-07-20)

The Source & build view skipped the service's `token` and `env` keys because they are
credentials — the right instinct, encoded as a list of two names. A private registry's
`source.password` was not on that list, so a real GitHub token was printed in full on
screen (v0.66.0). Meanwhile the EDIT form for the same field had always masked it: one
fact rendered two ways.

The rule: **judge a field by its NAME, not by membership of a list you maintain.** A list
only covers the secrets someone already thought of; a name test covers the ones EasyPanel
adds next. When adding any screen that dumps an object from the API, assume it contains a
credential you have not heard of.

### Two helpers one character apart (2026-07-20) — a class worth hunting

`centered_abs(pct_x, …)` takes a PERCENTAGE; `centered_abs_w(width, …)` takes COLUMNS.
Two dialogs computed an absolute width — carefully, with comments explaining the +4 for
borders — and passed it to the percentage one. The measurement did its job and the result
was thrown away, silently, in the direction that CUTS (68 → 54 at 80 columns). Nobody
noticed because at ~100 columns the two happen to agree.

The class: **a number handed to a parameter whose unit is different.** It cannot fail
loudly — both are `u16` — and it is invisible at whatever width the author happened to
test. When a function computes a size with care, check what the receiver thinks that
number means. Grep for sibling functions whose names differ by a suffix.

### Scale lesson: a real host has 713 domains (2026-07-20)

The owner's third host holds **713 domains and 100 services** — an order of magnitude
past anything the earlier screens were driven at, and it immediately produced a bug that
49 domains never would (see v0.64.1: filtering from the bottom left ONE row on screen
under a title claiming 452). When hunting for defects, drive the BIGGEST host, scroll to
the end, and then act — the interesting failures live where a list is longer than the
screen and then changes length underneath the view.

### Ideas parked for a future run (2026-07-20)

- **Uptime, round two: the background half.** The owner asked for both on-demand and
  continuous; v0.65.0 shipped the first. The second needs history — which is the first
  honest argument for real storage in this repo (see the storage ladder: memory → a JSON
  file → SQLite only for time-series we own). Before building it, decide deliberately
  whether it is a foreground command or a daemon, and USE the on-demand version for a
  few days first: if "check when I suspect something" turns out to be enough, the daemon
  should never be built at all. — judged by operator impact, not parity

Ranked by impact per unit of work. None of these needs an endpoint the tool does not
already call, which is why they are ideas rather than probes.

- ~~**Orphan domains**~~ done in v0.69.0. Measured first: 1 of 713 on the live host, not
  the flood the idea assumed — and it was still worth building, because one dead route
  among seven hundred is exactly what a person cannot find. **Measure before you build**:
  the number changed the design (a quiet mark and a count, not a whole screen).

- ~~**Diff two services**~~ done in v0.71.0 (same host, mark two), v0.72.0 (one service
  across hosts), and v0.74.0 (WHOLE project across hosts — surfaces services present on
  only one side plus per-service drift counts, two inspectProject calls). env by key,
  values never shown, order-independent. Fully shipped.

- ~~**Export a project's config to a file**~~ EXPORT done in v0.75.0 (`project export`) —
  redacted, stable, git-committable JSON; env keys only, token dropped, secret fields
  masked, volatile per-deploy noise excluded. **APPLY (import) is still open and is the
  valuable-but-risky half.** To build it, first probe: the export omits env VALUES and
  secrets by design, so an apply cannot recreate them — decide whether apply (a) only
  touches non-secret config (source/build/deploy/resources/domains/ports/mounts) and
  leaves env/secrets alone, or (b) takes a separate secrets file. Must be idempotent
  (updateX not createX where the service exists) and verified round-trip on a zzz-* project
  before trust. Medium-to-large.

- **Watch mode: turn crash visibility into an actual alert.** The tool already knows when
  a service is `down` (Swarm replicas missing) — that is the hard part, and it is done.
  A headless `easypanel watch` that polls and fires a webhook / desktop notification /
  non-zero exit would make it useful when nobody is looking at the screen. The brief
  already records that EasyPanel's own alerting is not exposed through the API, so this is
  squarely in "things the panel can't give you". Medium. Decide deliberately whether it is
  a foreground command or a daemon — a daemon brings state, and this repo has none.

### Growth backlog — richer features (every endpoint verified in 2.32.2)

Fill out the management surface. Verify live with a `zzz-*` target and clean up.

4. ~~**Port management.**~~ Add done v0.16.0, **delete** done v0.21.0 (in the Ports viewer,
   press `[0-9]` → confirm → `deletePort` by index; list reloads in place). Verified live
   that the index equals the listed position. **Open:** a Ports step in the create wizard
   (`createService` takes `ports` inline) — additive, low priority.
5. ~~**Mount management**~~ Add + delete done in v0.30.0, **edit done in v0.58.0**
   (`e` in the Mounts viewer → `updateMount {projectName, serviceName, index, values}`,
   values re-fetched at edit time, list reloaded on save), mirroring ports: `M` opens a
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
    - ~~**project env**~~ done in v0.62.0. `e` → "Project env" opens it in $EDITOR →
      `updateProjectEnv {projectName, env}`. The prefill doubt is RESOLVED: `inspectProject`
      DOES return it at `/project/env`; it only looked absent because the key does not exist
      until the env is first set. Two more facts, both verified live through the tool's own
      container shell: the variables DO reach the containers (a project variable sat next to
      the service's own inside a running container), and **a change does not take effect
      until the services are deployed again** — the container still reported the old value
      after the save, and the new one only after a deploy. So saving ends with an offer to
      deploy the services that went stale, counting only deployable types.

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
6. **A choice the user made that lives only in memory.** Switching host was applied to
   the session and never written to the config, so the next launch silently came back
   up on the old machine (v0.63.0, owner-reported). Whenever a deliberate act changes
   which thing you are pointed at, ask what the NEXT launch will do — a per-session-only
   answer is the same class as the wrong-machine mistake the server colours exist to
   prevent.
7. **Identity that cannot be corrected.** A server's name could not be edited, so a typo
   in it was permanent: the only fix was deleting the server, and its token cannot be
   read back from anywhere (v0.63.0). If a field is the label everything else identifies
   a thing by, it must be editable, and the correction must not risk the credential.
8. **A save that quietly takes no effect.** A project env change is stored immediately
   but running containers keep the old values until they are deployed (proven live,
   v0.62.0). Reporting "saved" and stopping there tells the user the change is live when
   it is not — say what is still stale, and offer the step that makes it real.

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

### Database backups — verified shapes (2026-07-20)

- `createDatabaseBackup` takes a SCHEDULE: projectName, serviceName, databaseName,
  schedule, enabled, storageProviderId, storageProviderPath.
- `runDatabaseBackup` takes only `{id}` — a schedule id. **There is no one-off backup
  endpoint**, hence create-disabled → run → delete (a disabled schedule runs fine; an
  earlier failure that looked like the `enabled` flag was the database not being up).
- `deleteDatabaseBackup` — note the name. Every other destructive op here is `destroy*`;
  `destroyDatabaseBackup` does not exist.
- `restoreDatabaseBackup` takes projectName, serviceName, databaseName,
  storageProviderId, `path` — so it restores INTO any database service, not only the one
  the backup came from. It recycles the container, so the service is briefly down.
- **Nothing lists backup FILES.** `listActions` does: a backup run records
  `meta: {databaseName, path, storageProviderId}`. The history is the file list.
- `storageProviders/common/list` is the ONLY storage endpoint — no create/update. A
  `local` provider stores on that host's disk, so cross-host restore needs a shared
  REMOTE provider configured in the dashboard on both hosts.
- A backup of a database that is not running fails with `Invariant failed`.
- **Redis cannot be backed up**: `createDatabaseBackup` answers `Service is not supported`,
  and redis has no `databaseName` field. mysql/mariadb/postgres/mongo are accepted.
  Careful reading the probe: a NONEXISTENT service answers with that same message, so
  only trust it against a service you know exists.
- **`databaseName` may be ANY database inside the service**, not just the one the panel
  created — verified by backing up a schema the panel never knew about and getting a real
  dump of it. Nothing in the API lists them, so ask the engine through `containerShell`
  (`SHOW DATABASES` / `pg_database`) and drop the bookkeeping schemas.

### Cross-host restore — verified on TWO live panels (2026-07-20)

- **Storage provider ids are PER-PANEL.** The same R2 bucket is `cmrs9o1r…` on one host
  and `cmrs9n1b…` on the other. Never carry an id across; resolve the destination's own.
- A `local` provider is unreadable from anywhere else — exclude those from a cross-host
  list and SAY how many were excluded.
- `restoreDatabaseBackup` on host B, with B's provider id and the path recorded by A,
  restores A's data. Proven: rows written on aurel were read back on angelia.
- A restore does NOT normally disturb the container. Measured directly: restoring on a
  live host and probing the restored container against an unrelated control at 10/30/60/
  120/180 s, both answered every time, the container never restarted, and the data was
  correct. An earlier run saw the shell AND the DB client go silent on angelia after
  several restores in a row — real, but a one-host observation, NOT what restoring does.
  The lesson is the method: when something looks broken after an action, probe an
  unrelated target at the same moment before blaming the action.

### Verified capability matrix per service type (probed live, 2026-07-20)

**Deploy block**: `services/app/updateDeploy` takes `{projectName, serviceName, deploy:
{replicas, command, zeroDowntime}}` — NESTED. A flat `{replicas: 3}` returns 200 and
changes nothing, so a wrong shape is indistinguishable from success. app only.

**Validation errors**: the useful text is in `data.zodErrors`, NOT the top-level
`message` (which only ever says "Input validation failed"). It is sometimes flat
(`{"name": "Required"}`) and sometimes nested under the payload's shape
(`{"values": {"name": "…"}}`). Volume names must be lowercase a-z, 0-9, `-`, `_`.

**Domains**: `createDomain` accepts ONLY web types (app, box, compose, wordpress) —
a database answers `Wrong service type.`. Note the panel is NOT atomic about it: the
domain appeared in `listDomains` even though the call 400'd, so a probe leaves a stray
record to clean up.

**Env**: redis has no `env` field until you set one, but `updateAdvanced` accepts and
stores it — so an absent field is not proof an action is unavailable. Check the write,
not the read.

**Mounts and ports**: `mounts/*` and `ports/*` accept ONLY `app` and `box`. Databases,
`compose` and `wordpress` answer `Invalid service type`. Checked with real services of
each type — create the throwaway first, because an absent service can answer the same way.


`-` = the route does NOT exist (bare `{"error":"Not found"}`); `yes` = it does (a bad
argument answers with a tRPC `NOT_FOUND`/400 instead). Probe with a nonexistent service
name — the two shapes stay distinguishable, cross-checked against a real service.

| operation           | app | compose | box | wordpress | mysql/mariadb/postgres/mongo/redis |
|---------------------|-----|---------|-----|-----------|------------------------------------|
| inspectService      | yes | yes     | yes | yes       | yes                                |
| deployService       | yes | yes     | -   | -         | -                                  |
| restart/stop/start  | yes | yes     | yes | yes       | -                                  |
| enable/disableService | - | -       | -   | -         | yes                                |
| destroyService      | yes | yes     | yes | yes       | yes                                |
| updateEnv           | yes | yes     | yes | yes       | -  (env lives in updateAdvanced)    |
| updateAdvanced      | -   | -       | yes | -         | yes                                |
| updateResources     | yes | -       | yes | yes       | yes                                |
| updateBasicAuth     | yes | yes     | yes | yes       | -                                  |
| updateRedirects     | yes | yes     | yes | yes       | -                                  |
| updateSource*/updateBuild/enableGithubDeploy | yes | - | - | -        | -                                  |

Ports, mounts and database backups are NOT under `services/*` at all — they have their own
groups (`ports`, `mounts`, `databaseBackups`), so their absence there is not a gap.

**Not every service type has the same verbs** (v0.52.0). `services/{type}/{action}Service`
was assumed to exist for all of them. It does not: databases (mysql, mariadb, postgres,
mongo, redis) have NO deploy/restart/stop/start route — they cycle through
`enableService`/`disableService` — and `box`/`wordpress` have no deploy. The tell is the
error SHAPE: a bare `{"error":"Not found"}` is a missing route, while a real operation
given a bad argument answers with a tRPC 400. Probe with that distinction before
assuming an endpoint exists for a type you have not tried. This one had been broken for
every database in the panel, and it silently disabled the Config File editor with it,
since a config change needs a process restart to take effect.

**A faithful copy can still be a broken one** (v0.51.1, resolved in v0.57.0 — the fix
is to COMMENT the two offending directives, not to drop the file: the owner wants the
configuration copied, and a commented line loses nothing). Cloning a MySQL replica
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
