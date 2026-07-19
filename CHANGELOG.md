# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.48.7] — 2026-07-19

### Fixed

- **The create-service wizard let you walk past every step it would later
  reject.** Press Enter through Basics, Source, Build, Environment and Domains
  with an empty **Name** and an empty **Repo** and nothing objected — until the
  final step, which refused with *"Service names may only contain a-z, 0-9, - and
  _"*: a complaint about a field four steps back and off screen, blaming the
  character set of a name that was simply missing. The form stayed on Domains,
  with no indication of where to go.

  Each step is now checked on the way **out**. An empty name stops you on Basics
  with "Give the service a name first"; an empty repo stops you on Source with
  "Repo must be selected". A refusal can no longer appear two steps away from the
  field that caused it, and it is drawn on the form's own border — beside the
  field it names — instead of in a status line that clears itself after six
  seconds.

  The Source step validates through the same builder that shapes the request, so
  the wizard cannot accept what the API would reject.

## [0.48.6] — 2026-07-19

### Fixed

- **The gauge percentage was unreadable once the bar reached it.** ratatui swaps
  foreground and background for the part of a gauge's label that sits on the
  filled bar, and the style set no background — so that half rendered as the
  terminal's *default* text colour on green: light on light in a dark theme. The
  Memory gauge at 54.4% was the reported case, and it fails exactly when the
  number is worth reading. The label now has a real colour on both sides of the
  boundary.

- **"CPU History (%)" was not showing percentages.** The sparkline rescaled each
  window to its own minimum and maximum, so the lowest sample was *always* an
  empty bar and the highest *always* a full one, whatever the real load. Measured
  against a live host: CPU moving between 7.80% and 19.35% — a quiet 11-point
  band — was drawn as a chart sweeping from empty to full under a panel titled
  "(%)". It read as a machine pegged at 100% while it idled.

  Percentage series (CPU, Memory, Disk) are now drawn at their true height. The
  axis adapts in steps and **says what it is** — `CPU History (0–25%)` — so an
  idle host stays readable without the chart claiming a load that isn't there.
  Network rates keep the window-relative scaling, which is the only sensible
  choice for a series with no ceiling, and their tiles print the actual rate.

## [0.48.5] — 2026-07-19

### Fixed

- **The Services table redrew in quadratic time.** Every frame, each row looked up
  its metrics by scanning the whole metrics list — two or three times per row —
  and its deploy state by scanning every recent action. At 500 services that
  measured **89.7 ms per frame**: about eleven redraws a second, with keypresses
  queued behind each one, on the screen the tool opens on.

  The lookups are now built once per frame instead of per row.

  | services | before | after |
  |---|---|---|
  | 50 | 3.19 ms | 2.25 ms |
  | 200 | 17.7 ms | 4.06 ms |
  | 500 | 89.7 ms | **7.94 ms** |

  The curve is near-linear rather than quadratic: ten times the services now costs
  about three and a half times the time, not twenty-eight.

- **The Monitor screen built its table twice per frame** — once for the rows and
  again, cloning the whole dataset, purely to count them for the title. 14.98 ms →
  9.17 ms at 500 services.

### Internal

- A benchmark (`bench_render_cost`, `#[ignore]`d) is kept in the test suite so this
  stays measured rather than assumed. Run it with
  `cargo test bench_render_cost -- --ignored --nocapture`. It disproved two
  plausible hypotheses about where the time went before pointing at the real one.

## [0.48.4] — 2026-07-19

### Fixed

- **A form's own instructions no longer disappear while you are still using it.**
  Guidance like `0 = unlimited`, `copies the config, NOT the data` and
  `to delete one instead: 'm', then its digit` was written to the status line —
  which reverts to "Ready" after six seconds. Open the resource form, think for a
  moment about what to type, and the sentence explaining what `0` means is gone.

  Each form now carries its own note, drawn on its bottom border, so it lasts
  exactly as long as the form does. Opening a form no longer writes to the status
  line at all: there is nothing left there to go stale, and the status line goes
  back to being what it is good at — a transient toast.

  The migrate form keeps its service count and its data warning on screen for the
  whole edit, which matters most there: it is the last screen before services are
  created on another host.

## [0.48.3] — 2026-07-19

### Fixed

- **The Actions list hid which service an action happened to.** The columns need
  88 characters once the spacing, highlight symbol and borders are counted, so on
  an 80-column terminal "Target" was squeezed from 28 to 20 and
  `harisenin-net-db/phpmyadmin` rendered as `harisenin-net-db/php` — a history of
  what happened, to something you cannot name.

  Whole columns are dropped instead, and **Duration goes first, not Age**: a
  history screen that cannot say *when* has lost the point of itself, while how
  long something took is one keypress away in the action detail. Age is given up
  only when even four columns will not fit. Wider terminals are unchanged.

### Internal

- The "drop whole columns rather than shrink them all" rule had three copies
  coming (Services, Hosts, Actions) and now has one, in `table.rs`, along with
  the reasoning: squeezed, ratatui shrinks every column proportionally, which is
  how `199.9 GB / 784.9 GB` became `199.9 GB / 784`. Landed as its own
  behaviour-preserving commit — identical test names before and after.

## [0.48.2] — 2026-07-19

### Fixed

- **The Monitor tiles showed figures that were wrong, not merely short.** Five
  tiles across an 80-column terminal leave 14 usable columns each, and the
  sub-line was simply cut to fit: Disk read `199.9 GB / 784` — a total with no
  unit, wrong by three orders of magnitude — Memory lost its unit the same way,
  and CPU read `16 cores — loa` with the load average gone.

  Each sub-line now offers a ladder of forms and the renderer takes the widest
  that actually fits. Memory and Disk shorten to `31.4/59.0 GB` and
  `200.0/784.9 GB`, keeping **both** halves — a lone `31.4 GB` would read as a
  complete figure while hiding that it is half of one. CPU falls back to the load
  average, then the core count. If nothing fits, nothing is drawn: a blank is
  honest, a cut number is not.

  Wider terminals are unchanged — the full `200.0 GB / 784.9 GB` returns as soon
  as there is room for it.

## [0.48.1] — 2026-07-19

### Fixed

- **The command palette was a wall of the same text.** Every action row carried
  the service it applied to — so opening `:` on a service produced thirty rows
  each ending in `·  harisenin-net/api`, and the actual action names were buried
  in the repetition. The service is now named once, in the palette's title
  (`Search: ▏ · actions for harisenin-net/api`), and each row is just the action:
  `Deploy`, `View env`, `Basic auth`.

  Searching is unchanged. The context moved out of the label but not out of the
  match, so `deploy core` still narrows to `Deploy` and `Auto deploy on/off` on
  that service even though "core" no longer appears in any row.

## [0.48.0] — 2026-07-19

### Added

- **`←`/`→` scroll the viewer sideways.** Logs, action output and config files
  open in a pane that neither wraps nor reflows, so anything past the right edge
  was simply unreachable — on the screen you spend the most time in. A log line
  reading `Running ['artisan' notifica` had the rest of it on the server and no
  way to see it. `Home` now returns to the first line *and* the left edge, and
  the pane says `← col 25 · Home to return` once you have moved, so a view
  missing its left edge can't be mistaken for content that starts there.

  **This changes what `←`/`→` do in the viewer.** They used to switch tabs — and
  that was the real trap: reaching for the rest of a cut line threw you out of
  the logs onto an unrelated screen, losing your place to reach text that was
  already there. `Esc` still returns to where you came from and `1`–`7` still
  jump to any tab, so nothing became unreachable.

### Fixed

- **The viewer no longer scrolls past its last line.** `Down` and `PageDown` had
  no upper bound, so holding either carried you into a blank bordered box that
  reads exactly like an empty log. The scroll position is now clamped on every
  path, not only while following a live tail.

## [0.47.1] — 2026-07-19

### Fixed

- **A deploy that was working reported itself as failed.** Roughly thirty seconds
  into any real deploy, the status line announced
  `Error: Deploy <project>/<service> failed: error sending request for url …`
  while the table showed that same service as `deploying` and the server built on
  to a successful finish. The tool contradicted itself at the exact moment you
  most need to trust it, and the obvious response — deploy again — is the wrong
  one.

  The cause was a request timeout being read as a verdict. `deployService` blocks
  until the build ends (measured: 51 seconds for a trivial Dockerfile, far longer
  for anything real), the client gives up at 30 seconds, and a proxy in front
  gives up around 125 seconds with a 524 — none of which cancels the deploy.

  A failure is now only reported when the server actually refused: a timeout or a
  gateway 5xx no longer produces a message, because the deploy is still running
  and the status column already tracks it. Genuine rejections still surface —
  deploying a nonexistent service (`400 Invariant failed`) or the wrong service
  type (`404`) reports exactly as before.

### Internal

- Server errors are now a typed `ApiError { status, message }` rather than a
  formatted string, so callers can tell a refusal from a gateway timing out
  without parsing the status back out of the message. The text users see is
  unchanged.

## [0.47.0] — 2026-07-19

### Added

- **`Enter` on a host opens its detail.** Hosts was the one screen with no row
  action at all: you could see that a server was `DOWN` and had no way to ask
  why, because the Status cell has room for about a dozen characters of the
  reason (`DOWN — error sen`). The detail view carries the whole thing, wrapped
  to the pane, and `Esc` returns you to the list. For a healthy host it shows the
  full figures — including the columns a narrow terminal has to drop.

### Fixed

- **The Hosts table no longer renders half a number as if it were whole.** The
  columns needed 123 characters plus the highlight symbol, and below that ratatui
  shrinks every column proportionally — so on an 80-column terminal
  `29.8 GB / 59.0 GB` was drawn as `29.8 GB`. That is not a cosmetic truncation:
  it reads as a complete memory figure and is off by half. Whole columns are now
  dropped instead, least useful first (URL, then Load, then Disk), and Status —
  which carries the failure reason — always survives and takes the freed space.

  The thresholds count what is easy to forget: the space between each pair of
  columns, the two-column highlight symbol, and the borders. The first attempt
  used round numbers and still cut Disk to `194.7 GB / 784.9`; it was caught by
  looking at the screen, not by the tests.

- **Long words no longer overflow a wrapped pane.** The shared word-wrapper broke
  on whitespace only, so a single long token — a URL, a stack frame — ran past
  the edge and was cut. It is now split across lines.

## [0.46.1] — 2026-07-19

### Fixed

- **The status line no longer reports "Ready" over a request that is still
  running.** "Is anything happening?" was inferred from the message text ending
  in `...`, and a six-second timer rewrote that text to "Ready" whether or not
  the reply had arrived. The worst case was `systemPrune` — a host-wide,
  irreversible Docker prune whose only feedback is "Sending..."; six seconds
  later the bar claimed it was done while the request was still in flight, and
  re-running it is the obvious next move.

  The worker's user lane now counts what it is actually working on, and both the
  spinner and the fade read that count. The periodic metrics lane is deliberately
  not counted: it refetches every two seconds and would pin a spinner on screen
  permanently while telling you nothing about the action you asked for.

  Seen against an unreachable host: `⠧ Loading…` held for 27 seconds without
  fading, then the connection error appeared and stayed. Before, the bar read
  "Ready" with no spinner while three requests hung.

- **The spinner stopped guessing.** It ran whenever the message happened to end
  in `...` — so it kept spinning after a reply had come back, and stopped the
  moment an unrelated message replaced the text. It now runs exactly while
  something is in flight.

- **"Ready" no longer appears next to a running spinner.** During the initial
  load the resting message sat beside a spinning indicator, each contradicting
  the other; the line now reads "Loading…" while that first fetch is out.

### Internal

None of the ~68 request dispatch sites changed. Counting inside the worker,
which is the only place that knows when work actually ends, kept this to one
file's plumbing instead of a rewrite of every call site.

## [0.46.0] — 2026-07-19

### Added

- **Force rebuild is now reachable from the TUI** — Lifecycle (`d`) → **Force
  rebuild (no cache)**. The CLI has supported `service deploy … --force` all
  along, but the TUI sent `forceRebuild: false` as a hard-coded literal, so no
  deploy started from the interface could ever skip the layer cache. When a build
  picks up a stale dependency, the only way out was to drop to the CLI.

  It is a separate menu entry rather than a change to Deploy: ignoring the cache
  can turn a seconds-long deploy into minutes, so it should be a choice you made
  rather than a surprise. The confirmation says what it will do — "Rebuild 'app'
  from scratch, ignoring the build cache?" — and the same "previous deploy still
  running" warning applies.

  Verified live on a two-step Dockerfile build: an ordinary deploy reported both
  `RUN` steps `CACHED`, while the force rebuild re-ran both — first against the
  API directly, then again end-to-end through the menu.

## [0.45.1] — 2026-07-19

### Fixed

- **A failure no longer erases itself six seconds later.** The status line is the
  only place an error is ever shown — there is no log and no history to scroll
  back to — and a blanket timer replaced every message with "Ready" once it had
  been on screen for six seconds. Look away while a request is in flight and the
  reason it failed is gone for good, replaced by a claim that everything is fine.
  Errors now stay until your next action replaces them; routine notices ("Deploy
  started", "Env saved") still fade as before.

  Seen on a live host: `Error: [400] Project already exists.` was on screen at two
  seconds and gone at eight.

- The rule for what counts as a failure now has **one definition** instead of two.
  The renderer used its own copy to pick the error colour and the event loop used
  another to decide what to erase, so a message could be painted red as an error
  and then quietly discarded as if it were a routine notice.

### Known limitation

The status line still cannot distinguish "working" from "finished": the spinner is
derived from the message text rather than from a real in-flight count, so a
long-running action can report "Ready" before its request comes back. Fixing that
properly needs request tracking across every dispatch site and is tracked in
`.github/AGENT_BRIEF.md` rather than patched with a heuristic — the obvious
heuristic was tried and leaves a spinner running forever on screens that refresh
without sending anything.

## [0.45.0] — 2026-07-19

### Fixed

- **GUI editors no longer discard your edit.** Setting `EDITOR=code` (or Cursor,
  Zed, Sublime, VSCodium, Windsurf, or a JetBrains IDE) looked supported but
  quietly threw work away: those editors hand the file to an already-running
  window and exit immediately, so the TUI read the temp file back before a single
  keystroke was typed, found it unchanged, reported "Unchanged", and deleted it.
  The flag that makes them block until the file is closed is now added
  automatically — `EDITOR=code` runs as `code --wait`. An editor that already has
  it (`EDITOR="code -w"`) is left alone, and terminal editors, which block on
  their own, are untouched.

### Added

- **`EASYPANEL_EDITOR`** — checked before `$VISUAL` and `$EDITOR`, so you can use a
  terminal editor here without changing the editor the rest of your machine uses.
- While a GUI editor is open, the terminal now says what it is waiting for. The TUI
  is torn down for the hand-off, and a blank screen with no explanation reads as a
  hang.

## [0.44.0] — 2026-07-18

### Added

- **Migrate a service, or a whole project, to another EasyPanel host.** Moving
  between hosts is the thing the web panel makes hardest: there is no export and
  no import, so today it means retyping every service by hand. Pick a destination
  server and a target project (created there if it doesn't exist) and the
  configuration goes over — image/source and build settings, env, mounts, ports,
  resources, a database's advanced config file and its credentials, and the
  domains. Available on a service row, and on a project header row for every
  service in it at once.

  **It moves configuration, never data.** Volume contents and database rows live
  on the origin host's disk and are not reachable through the API, so they stay
  behind; move them yourself (`mysqldump`, a volume copy). Every message says so
  rather than letting the word "migrate" imply more than happened. Domains are
  recreated pointing at their existing hostnames — the DNS cutover stays yours to
  time. One service failing does not abandon the others: the result reports what
  landed, what failed, and why.

- **A project header row now has its own action menu** (`Space`). It previously
  opened nothing at all, which left project-wide migration unreachable from the
  row users would naturally try it on. It offers migrate-whole-project, new
  service, new project, and destroy project — surfacing `N` and `X`, which until
  now existed only as undiscoverable keys.

### Fixed

- **Form footers no longer cut a keyboard hint mid-word.** The form is sized as a
  percentage of the terminal, so on an 80-column screen the hint line overflowed
  and rendered as `[Esc] can` — the escape hatch, mangled, for the user most
  likely to need it. Hints are now dropped whole, most important first, so what is
  shown is always complete.

### Internal

- The copy-a-service rule moved into a `migrate` domain module and now has one
  definition instead of two. Cloning is migration with the same host on both
  sides, so it delegates there. The duplicate had already drifted once, losing
  registry credentials on one path but not the other.

## [0.43.3] — 2026-07-18

### Fixed

- **The action menu read as corrupted text.** The popup sat flush against the row
  behind it, so the table continued hard at its border — `┐d`, `│dio-db (5)` — which
  looks like a rendering fault rather than a menu floating over a table. There is now
  a blank column to its right. Only to the right: the column on its left carries the
  `›` marker for the row the menu acts on.
- **The command palette closed in silence when nothing matched.** Pressing `Enter`
  on a query with no results simply took the palette away — indistinguishable from
  having run something. It now says `Nothing matches '<query>'`.

## [0.43.2] — 2026-07-18

### Fixed

- **The help overlay hid most of itself on a short terminal.** At 80x24 it simply
  stopped at the bottom border: the *Anywhere*, *Inside forms & dropdowns* and
  *Mouse* sections were entirely invisible, with nothing on screen to suggest more
  existed — and every description was cut mid-word (`Env menu — view / edit / r`),
  including the one documenting `:`, the newest feature. Help that lies by omission
  is worse than no help. It now wraps instead of truncating (continuation lines stay
  aligned under the description), scrolls with `↑↓`/`PgUp`/`PgDn`/`Home`/`End`, and
  its border shows the position and how to leave — any other key still closes it.
  The key column is also capped, so one long entry no longer squeezes every
  description into a narrow gutter.

## [0.43.1] — 2026-07-18

### Fixed

- **Confirmation dialogs hid what you were confirming.** The box was sized as a
  percentage of the screen, so on an 80x24 terminal it came out 41x5 for six lines
  of text — and the paragraph never wrapped. The prune confirmation rendered as
  `Prune the Docker system? Unused containe`, cut mid-word, and the
  `[y] Yes  [n] Cancel` line fell off the bottom entirely: an irreversible,
  host-wide action approved without being able to read it or see which key
  confirms. The dialog is now sized from its own content and wraps instead of
  truncating, so the question, the blast radius and the keys are always visible.

## [0.43.0] — 2026-07-18

### Added

- **A `Repl` column in the Services table** — how many replicas each service runs,
  the number the web panel keeps behind Deploy → Replicas. It shows Swarm's live
  count, and while that count differs from the target it shows both (`0/1` for
  replicas that never came up, `1/3` mid-rollout), which is the moment the number is
  worth looking at. Falls back to the configured `deploy.replicas`; `-` for a service
  with no deploy block. No extra API call — the data was already being polled.

### Changed

- **The Services table now adapts to the terminal width.** Below ~120 columns the
  four metric columns are dropped instead of squeezed: at 80 columns the table used
  to render `Statu`, `● act` and metric slivers like `0.` and `77`, which is worse
  than not showing them. Identity, status, replicas, source and auto deploy always
  survive; the metrics remain on the Monitor tab.

## [0.42.3] — 2026-07-18

### Fixed

- **Cloning a service silently dropped its registry credentials.** A service
  pulling from a private registry carries `username`/`password` on its source; the
  clone path sent only the image, so the clone could never pull — while the status
  bar reported a successful clone. The failure only surfaced later, at deploy.
  Verified end-to-end against a live server: the original kept its credentials, a
  clone taken before this fix had none, a clone taken after has them.

### Internal

- The rule for *which `updateSource*` endpoint a source type uses and which keys its
  body carries* now lives in one place (`src/source.rs`) instead of being duplicated
  between the create/edit form and the clone path. The two copies had already
  drifted — that drift is what lost the credentials. The form keeps its own
  validation and field labels; only the payload shape is shared.

## [0.42.2] — 2026-07-18

### Fixed

- **The action menus looked broken on first contact.** Opening Services put the
  highlight on row 0 — which is a *project header*, not a service. Every service
  action reads the selected row, and a header has none, so `Space`, `e`, `o`, `u`,
  `m`, `d`, `t` and `x` all did nothing at all, with no message, until you pressed
  `↓`. The headline feature of the last few releases appeared dead on arrival. The
  first paint now lands on the first service.
- **Row actions no longer fail in silence.** With a project header selected, the
  group keys used to open a menu whose every item quietly did nothing. They now say
  `Select a service first` instead — the same wording the other row actions already
  used.

## [0.42.1] — 2026-07-18

### Fixed

- **Editing env on a database service used to fail silently.** `updateEnv` does not
  exist for `mysql`/`mariadb`/`postgres`/`mongo`/`redis` — the server answers
  `Not found` (verified live). A database keeps its env in the Advanced block, so
  `E` / `w` on one never saved anything. Env now routes by service type: `app`,
  `box`, `compose` and `wordpress` use `updateEnv` (preserving `dotEnvPath`), every
  other type uses `updateAdvanced` (preserving `image`, `command` and the config
  file). Covered by two tests, one per path.

## [0.42.0] — 2026-07-18

### Changed

- **The entire codebase and interface are now in English.** Every user-facing
  string — status messages, the help overlay, menu and palette labels, form field
  labels, service status words (`active` / `stopped` / `down` / `disabled` /
  `deploying`) — plus all source comments were translated from Indonesian to
  natural English, with consistent terminology, punctuation, and sentence case
  throughout. This makes the project approachable for the wider community it is
  meant for. No behaviour changed; the full test suite passes (form field lookups
  that key off label text, and the status/error strings the tests assert, were
  renamed in lockstep).

## [0.41.0] — 2026-07-18

### Changed

- **Command palette actions are now context-aware and complete.** In 0.40.0 the
  palette listed a fixed set of actions for *every* service (hundreds of entries,
  and only lifecycle verbs). Now it shows actions for the **currently selected
  row only**, and the **full** set for it — for a service that means everything
  its menu offers (env view/edit/replace/toggle, domain, ports, redirects, basic
  auth, source, build, auto-deploy, resources, mounts, backups, lifecycle,
  terminal, DB shell, config file, clone, delete). The same applies to other
  screens: on Domains a selected domain contributes Edit / Set primary / Delete,
  on Actions the selected row contributes View detail. With no row selected the
  palette is pure navigation. Multi-word search still applies, so `deploy karir`
  jumps straight to that action.

## [0.40.0] — 2026-07-18

### Added

- **Quick actions in the command palette (`:`)** — the palette is no longer just
  navigation. Every service now also carries action entries — **Deploy, Restart,
  Stop, Start, Logs, Terminal**, plus **DB shell** for databases — so you can run
  them from anywhere without touching the menu (e.g. type `deploy pay` → `Enter`
  to deploy that service, with the usual confirmation). Lifecycle actions still
  go through their confirm dialog; the service is selected first so the action
  targets the right row.
- **Multi-word palette search.** The query now matches on each word independently
  (all must appear), so `deploy pay` finds `Deploy  …/pay` even though the words
  aren't adjacent in the label.

## [0.39.0] — 2026-07-18

### Added

- **Global search / command palette (`:`)** — a fast, keyboard-only way to jump
  anywhere without hunting through menus. Press `:` from any screen, type to
  fuzzy-filter across every service (project / name / type) and every tab, then
  `Enter` to jump straight there (a service selects its row on the Services tab).
  `↑↓` to pick, `Esc` to close. This is the alternative for operators who would
  rather not navigate via the context menu.

## [0.38.0] — 2026-07-18

### Added

- **Edit a database service's Config File** (the dashboard's Advanced → Config
  File — e.g. a MySQL `[mysqld]` block for replication). On a db service, the
  Build & source menu (`u`) now has **Config file (Advanced)**, which opens the
  current `configFile` in `$EDITOR` and saves it via `updateAdvanced`. Previously
  this was only reachable from the web panel. The save is read-modify-write: the
  required `image`/`command` and any existing `env` are preserved (verified live
  against the server), so editing the config never wipes the rest of the Advanced
  block.

## [0.37.0] — 2026-07-18

### Changed

- **Service actions are now grouped into menus instead of ~25 scattered single
  keys.** Related actions live under one entry: `e` → Env (view / edit / replace /
  toggle .env file), `o` → Networking (domain / ports / redirects / basic auth),
  `u` → Build & source (source / build / auto-deploy / resources), `m` → Storage
  (mounts / backups), `d` → Lifecycle (deploy / restart / stop / start), `t` →
  Shell (terminal / DB shell), `x` → Danger (delete service / project). This is
  the fix for the UX complaint that having a separate shortcut for every action
  (e.g. four just for env) is confusing and inefficient. The old leaf keys
  (`E`/`w`/`.`/`P`/`f`/`F`/`H`/`U`/`B`/`A`/`L`/`M`/`R`/`S`/`T`/`X`/`y`) still work
  for anyone with the muscle memory.
- **Menus navigate with arrow keys.** `↑↓` move, `→` enters a submenu (or runs the
  item), `←` goes back to the parent menu (or closes at the top level), `Enter`
  runs, `Esc` closes. Works from both the keyboard openers and right-click.
- **`←`/`→` switch tabs** (e.g. Services ↔ Domains), alongside `Tab` and `1`–`7`.
- **`Space` opens the row action menu** — the keyboard equivalent of right-click,
  so the full grouped menu is reachable without a mouse.

### Fixed

- **Keyboard-opened menus now appear at the selected row** instead of the top-left
  corner of the table, so the menu shows up in the context of the row it acts on
  (and no longer bleeds the underlying project names along its left edge).
- **Form labels no longer collide with their values.** The label column was a fixed
  width, so a long label (`Install command`, `Buat file .env`) ran straight into its
  value with no gap; the column now sizes to the longest label in the form.
- **The help overlay no longer jams the key against its description** for the same
  reason — the key column now sizes to the widest key shown.

### Internal

- Menu items now carry their action as a function, so the menu is a single
  definition of each action rather than simulating a key press — this also removes
  the previous drift where the right-click menu silently omitted ~13 actions the
  keyboard had.

## [0.36.0] — 2026-07-18

### Added

- **Deploy visibility.** The Services table now shows a **`deploying`** status
  (cyan) for any service with a deployment in progress, and the title counts them
  (`· ⚙ 2 deploying`). Before this, a running deploy left the old container up, so
  the row read `aktif` — indistinguishable from idle — and the "Deploy dimulai"
  message vanished after ~6s. An operator could re-trigger the same build over and
  over with no sign it was already running. The state is joined live from
  `listActions` (status verified against the server: `pending → running →
  done/error`).
- **Deploy debounce hint.** The deploy confirmation now warns
  (`⚠ deploy sebelumnya masih berjalan`) when a deployment for that service is
  still pending/running, so a second build is a deliberate choice, not an accident.

### Fixed

- **Immediate deploy failures are no longer swallowed.** `deployService` is
  dispatched off-thread (builds exceed proxy timeouts), but its result was
  discarded — an instant rejection (bad config, 400, non-deployable service) never
  reached the screen while the UI already said "dimulai". The worker thread now
  reports such failures to the status bar.
- **The Actions tab refreshes live.** It used to load once and then freeze until
  `r`; it is now polled while open (and while the Services table is shown), so
  deploy/action state you switch over to check is current.

## [0.35.0] — 2026-07-18

### Added

- **Toggle the `.env` file (`.`)** on app services. Turn on/off writing the env
  as a `.env` file inside the container (`dotEnvPath`) — previously this could
  only be set once, at service creation. Press `.` to flip it: enable (written to
  `.env`) or disable. The state is read and then inverted on the server, so the
  existing env is left untouched.

### Fixed

- **Editing env no longer silently disables the `.env` file.** `updateEnv`
  replaces the entire env configuration; previously `E`/`w` sent the env without
  `dotEnvPath`, so a service that had a `.env` file lost it on every env edit. The
  existing `dotEnvPath` is now preserved automatically.

## [0.34.0] — 2026-07-18

### Added

- **Fast wholesale env replace (`w`)**, alongside the existing edit. Two clear paths now:
  `E` **edits** a service's env — it loads the current variables into your `$EDITOR` so you
  can change a few and save — and `w` **replaces the whole env** by opening an *empty*
  editor to paste a fresh `.env` into, skipping both the fetch and the "clear the old
  content first" step. Saving an empty replace is treated as cancel, so `w` can't wipe your
  env by accident. Both save through `updateEnv` (which replaces the full env string); pick
  the one that matches the task.



### Added

- **One-key database shell — a login prompt you never have to type credentials for.** Press
  `y` on a **mysql, mariadb, postgres, mongo, or redis** service (or right-click → DB shell)
  and it drops you into that database's own client — `mysql`, `psql`, `mongosh`,
  `redis-cli` — already logged in as root/superuser, in the embedded terminal pane. The tool
  reads the stored credentials from the service and launches the right client for the type;
  the password goes through an env var (`MYSQL_PWD`/`PGPASSWORD`/`REDISCLI_AUTH`), so it
  never shows up in the process list or a warning. The web panel has nothing like it — you'd
  normally open a shell, remember the client and flags, and copy-paste the password.

  Every command shape was verified live against the running server: `mysql` (SELECT VERSION
  → 8.0.46), `psql` (SELECT version → PostgreSQL), `mongosh` (connected with
  `authSource=admin`), and `redis-cli` (PING → PONG). Credentials are shell-quote-escaped so
  an apostrophe in a password can't break the command.



### Added

- **Redirect rules from the TUI.** On a web service (app/box/compose/wordpress), `f` shows
  its redirects and `F` adds one — a source **regex**, a **replacement** (with `${1}`-style
  groups), **301 vs 302**, and enabled/disabled. In the redirects view, press a rule's
  number (`[0]`–`[9]`) to delete it after a confirmation. EasyPanel has no per-rule endpoint,
  so add and delete read the current list, change it, and write the whole array back via
  `updateRedirects` — verified live that adding two rules keeps both and delete-by-index
  removes the right one, so existing rules are never clobbered.



### Added

- **Basic auth — password-protect a web service from the TUI.** Press `H` on an app/box/
  compose/wordpress service (or right-click → Basic auth) to set a username and password
  behind which the service's HTTP endpoints sit; the form is pre-filled with the current
  credential so you can change it, and clearing both fields removes the protection. Backed
  by `updateBasicAuth`; verified live that both setting and clearing round-trip through
  `inspectService`. (Database services don't have HTTP auth, so the key is a no-op there
  with a note.)

## [0.30.1] — 2026-07-18

### Changed

- **The status bar is a single line again — just the message.** The keybinding line was
  removed: the full, always-current shortcut list already lives in the `?` help overlay, so
  repeating it at the bottom was redundant and cost a row of the table. The status/result
  message (with its spinner and error colouring) stays; the filter prompt still shows how to
  apply or cancel while you type. Removed the now-dead width-fitting helper and its test.

## [0.30.0] — 2026-07-18

### Added

- **Mount management from the TUI.** Press `M` on a service to add a mount — a **volume**
  (named), a **bind** (host path), or a **file** (inline content edited in `$EDITOR`) — the
  right fields appear per type. In the Mounts view (`m`), press a mount's number (`[0]`–
  `[9]`) to delete it after a confirmation; the list reloads in place. Until now the TUI
  could only *view* mounts (the CLI had `mount-add` but the TUI didn't). Verified live that
  all three mount shapes create and that delete-by-index removes the listed row.

### Fixed

- **Adding a domain from a service now pre-fills that service.** After opening a service's
  domains (`o`), pressing `n` (new domain) starts with the domain pointed at *that* service
  (its project and name), instead of a blank/arbitrary project — the whole point of coming
  from the service.
- **`Esc` from a service's domains returns to Services.** Opening domains from a service
  (`o`) filters the Domains tab to it; `Esc` now goes back to the service you came from
  rather than just clearing the filter and stranding you on the Domains tab.

## [0.29.0] — 2026-07-18

### Changed

- **Manage a service's domains, not just view them.** `o` on a service (or right-click →
  Domain) now opens the Domains tab filtered to that service, where the full domain toolset
  already lives — `n` add, `e` edit, `x` delete, `P` set primary — instead of the old
  read-only list. The filter matches the domain's destination (`…{project}_{service}…`), so
  you see exactly that service's domains and can act on them. This closes the gap where a
  service could only *show* its domains with no way to add, edit, or remove one.

## [0.28.1] — 2026-07-18

### Added

- **Form fields are clickable.** Click a field to focus it; clicking a yes/no field toggles
  it, a dropdown field opens its list, and an editor field opens `$EDITOR` — text fields
  just take focus so you can type.

### Fixed

- **Scrolling no longer nudges the selection when the whole list already fits on screen.**
  The earlier scroll fix moved the highlight even when there was nothing to scroll (a list
  shorter than the pane), so scrolling near the bottom shifted the selected row. Scroll now
  moves the selection only by however far the view actually scrolls — none, when everything
  is already visible.

## [0.28.0] — 2026-07-18

### Fixed

- **Scrolling a table no longer fights the follow-the-cursor selection.** The wheel now
  scrolls the viewport and moves the highlight together, so the selected row stays under the
  pointer instead of jumping — previously scroll moved the selection one way while the next
  mouse motion snapped it back, which was especially jarring on a trackpad. Scrolling in the
  log/detail viewer still scrolls its text.
- **Dropdowns are now mouse-driven.** An open dropdown (project picker, service picker,
  repo/branch, etc.) highlights the option under the cursor, selects on click, navigates on
  scroll, and closes on a click outside — before, it only responded to the keyboard.

## [0.27.0] — 2026-07-18

### Added

- **Clone can target a different parent project.** The clone form now has a Project dropdown
  (defaulting to the source's project) alongside the new name, so you can copy a service
  into any existing project — not only the one it came from. Only existing projects are
  offered, since a brand-new project's Docker network isn't ready the instant it's created;
  make the project first, then clone into it. Verified live that a cross-project clone lands
  in the chosen project with its config intact.

## [0.26.0] — 2026-07-18

### Added

- **Clone a service — a feature EasyPanel's own web panel does not have.** Press `c` on a
  service (or right-click → Clone) to create a new service with the **same configuration**:
  image/source, build, env (including credentials), resources, mounts, ports, deploy
  settings, and — for databases — the advanced config file. You name the copy; it lands in
  the same project and does **not** deploy or copy any data, so it is instant and safe.

  The motivating case is spinning up a **MySQL replica for replication**: cloning carries
  over the image, env, root/user passwords, and the `my.cnf` advanced config (server-id,
  log-bin, etc.) in one step, instead of re-entering all of it by hand.

  EasyPanel has no clone endpoint, so this is composed from ones it does have —
  `inspectService` → `createService` (with everything inline except the source, which would
  trigger a deploy) → `updateSource*` for app services / `updateAdvanced` for databases. The
  composition was verified field-by-field against the live server for both an `app` service
  (source, build, env matched) and a `mysql` service (image, env, passwords, and the
  replication `configFile` matched), using throwaway targets that were then cleaned up.

## [0.25.0] — 2026-07-18

### Added

- **Action detail view.** Press `Enter` on a row in Actions (or right-click → View detail)
  to open its full record — type, status, target, timestamp, and the **deploy/action log**
  — in the viewer, the same way the web panel's "View" button works. `Esc` returns to
  Actions (not Services). Backed by `getAction`, whose `log` field carries the output.

### Fixed

- **Switching servers no longer throws you back to Dashboard.** It now keeps you on the
  screen you were on (e.g. Services) and reloads that screen's data for the new host. Only
  the derived Viewer/Terminal screens fall back to Services, since their content belonged to
  the old server.
- **Mouse selection now follows the cursor.** Moving the mouse over a table highlights the
  row under it (not only on click), and hovering the right-click context menu highlights the
  item under the cursor — previously the highlight didn't track the pointer. Row hit-testing
  is also bounded on all sides now, so a hover on the border or just outside a table no
  longer selects a stray row.

## [0.24.0] — 2026-07-18

### Added

- **A colored status dot in the Services table** — each service's Status now leads with a
  `●` you read at a glance before the word: green `aktif`, yellow `berhenti`, gray `mati`,
  and red for `turun` (which keeps its pulse). The Auto column's `✓`/`✗` are colored to
  match (green on, gray off). Deliberately plain single-cell Unicode symbols, not emoji or
  Nerd-Font glyphs: emoji are often double-width and would break the table's fixed columns,
  and Nerd-Font icons render as tofu without a patched font — the dot works in every
  terminal and doesn't depend on the theme (palette-indexed colors, per this project's
  standing rule).

## [0.23.0] — 2026-07-18

### Added

- **Click any table row, not just Services.** Row selection by click now works on every
  table screen — Services, Domains, Actions, Monitor, and Hosts — matching where the
  keyboard already lets you select. (v0.22.0 shipped click-to-select on Services only.)
- **Right-click context menu.** Right-click a row to select it and open a small menu of the
  actions available for it — on a service: Logs, Terminal, Deploy, Restart, Stop, Start,
  Env, Resource, Delete; on a domain: Edit, Set primary, Delete. Navigate with the mouse or
  arrows, activate with click or Enter, dismiss by clicking away or Esc. Each item runs the
  exact same code path as its keyboard shortcut, so there is no second action path that can
  drift from the keys — and every action keeps its usual confirmation (delete still asks
  first). The overlay (`?`) now lists the mouse actions too.

### Changed

- The stored table area is now generic across screens (one field, since only one screen
  renders per frame), so click-to-select and the context menu work uniformly without
  per-screen bookkeeping.

## [0.22.0] — 2026-07-18

### Added

- **The TUI is now clickable.** Click a tab to switch to it, click a service row to select
  it, and use the scroll wheel to move through any table or scroll the log/detail viewer.
  Mouse and keyboard work interchangeably — nothing that used to work by key stops working.
  (Trade-off: capturing the mouse turns off the terminal's own click-drag text selection;
  hold **Shift while dragging** to select/copy text in most terminals.)

- **Motion that means something — four animations, each there to communicate, not decorate.**
  - **Loading spinner** in the status bar whenever an operation is in flight (a fetch, a
    save, a cross-service log search). A long wait now visibly *works* instead of looking
    frozen — the problem the rest of the tool already guards against, now shown.
  - **Down services pulse.** A service in `turun` (crashed / missing replicas) gently
    pulses red so your eye lands on the incident immediately.
  - **Tab switch flash** and **selection flash** give a brief, deliberate emphasis when you
    change tabs or move the highlighted row, so the change registers. (A terminal is a cell
    grid — highlights can't slide between cells, so these are honest short transitions, not
    faked smooth motion.)

  Animation only speeds up the redraw loop while something is actually animating; an idle,
  healthy screen stays at its old cheap refresh rate.

## [0.21.0] — 2026-07-18

### Added

- **Delete a port from the TUI.** Open a service's ports (`p`) and press the port's number
  (`[0]`–`[9]`) to remove it, after a confirmation. The list reloads in place, so the
  deleted port disappears immediately instead of lingering until you reopen the view — the
  same round-trip discipline the rest of the tool follows. This closes the port-management
  gap: since v0.16.0 you could add a published port but had no way to remove one without
  the web panel.

  Verified live against the running server that `deletePort`'s index is the position shown
  in the list: with ports `[0] 8080→80` and `[1] 9090→90`, deleting index 0 removed 8080
  and left 9090 (which then renumbers to `[0]`, so consecutive deletes stay correct).
  Tested with a throwaway `zzz-*` service, then cleaned up.

## [0.20.0] — 2026-07-18

### Added

- **Set CPU and memory limits per service, from the TUI (`L`).** Press `L` on any service
  and a form opens pre-filled with its current limits — CPU limit / reservation (in cores,
  decimals allowed) and memory limit / reservation (in MB). `0` means unbounded (EasyPanel's
  own convention). It works on every service type, not just `app`, because the endpoint
  group follows the service type (`services/{type}/updateResources`). Saving stores the
  config; deploy (`d`) applies it — the same store-then-deploy model as ports, so nothing
  restarts unexpectedly.

  Why it matters: on a host with dozens of services, one runaway container can starve the
  rest. Until now the only way to cap a service was the web panel; the tool could show you
  a service eating CPU but not do anything about it. Now the fix is one key away, across
  every host.

  Verified live against the running server: `updateResources` round-trips the exact numbers
  it is sent — including the decimal form the tool emits (`cpuLimit: 0.5`, `memoryLimit:
  1024.0`) — confirmed by reading `inspectService` back. Units mirror the EasyPanel
  dashboard (cores, MB); the swarm-level translation is EasyPanel's own and was not
  independently re-derived. Tested with a throwaway `zzz-*` service, then cleaned up.

## [0.19.2] — 2026-07-18

### Changed

- **The bottom status bar is now two lines: the message on its own line, keybindings
  below it.** Before, the status/result message and the whole keybinding list shared one
  row, so a long message (an error, a "deploy dimulai…" note) competed with the shortcuts
  and could be clipped at the right edge — you couldn't read it in full. Now the message
  gets a dedicated line and is never truncated by the shortcuts.
- **The keybinding line is width-aware.** It fits as many of the screen's shortcuts as the
  terminal is wide, always keeping `? bantuan · q keluar` at the end, and drops the rest
  at a `·` boundary (never mid-word) — the full list is always in the `?` help overlay. On
  a narrow terminal the bar no longer overflows or cuts a shortcut in half.

## [0.19.1] — 2026-07-18

### Changed

- **`install.sh` no longer needs `sudo` by default.** The old default target
  `/usr/local/bin` is on macOS's PATH but root-owned on Apple Silicon, so a plain
  `./install.sh` failed with `install: … Permission denied`. It now installs to
  `/usr/local/bin` only when that directory is actually writable, and otherwise falls
  back to `~/.cargo/bin` — which is guaranteed to exist (the script just ran `cargo`) and
  is already on PATH via rustup. `PREFIX=…` still overrides. `~/.local/bin` is
  deliberately not the fallback: it is not on macOS's default PATH, so installing there
  would silently produce `command not found`.

## [0.19.0] — 2026-07-18

### Added

- **Crash visibility — the Services table now tells you what's broken right now.** A new
  `turun` status, shown in red, marks any service whose Docker Swarm replicas are missing
  (`desired > 0` but `actual < desired`): a container that crashed or is stuck in a
  restart loop and has not come back up. The table title also counts them
  (`Services (33) · ⚠ 2 turun`), so a broken service is visible at a glance without
  reading every row.

  This closes a real blind spot. Until now a crashed service and a service you stopped on
  purpose both showed `berhenti` — indistinguishable, even though one is an incident and
  the other is intentional. The status is now derived from Swarm's own truth
  (`getDockerTaskStats`, one call covering every service), which knows how many replicas
  *should* run versus how many actually do — a stronger signal than "does it have
  metrics". A service scaled to zero (`desired = 0`) stays `berhenti`; only genuinely
  degraded services turn red.

  Verified against the live host: the replica map's keys match the tool's
  `{project}_{service}` join for all 33 services (zero misses), and a deliberately
  crash-looping throwaway service reported `actual=0, desired=1` and classified as `turun`
  as designed. Then cleaned up.

## [0.18.0] — 2026-07-18

### Added

- **Remote terminal into any container — embedded right in the TUI.** Press `t` on a
  service and an interactive shell to its running container opens **inside the content
  pane** — the tabs and status bar stay put, so it feels like part of the app, not a
  takeover. Type `exit` (or Ctrl-D) to close it; Ctrl-Q force-quits back to the table.
  Ctrl-C, arrow keys, tab-completion and colours all work.

  This is the feature that makes the tool more than a panel: a real shell into
  production, across every host, without leaving the terminal or opening a browser. It
  speaks EasyPanel's own WebSocket (`wss://{panel}/ws/containerShell`), authenticated
  with the API token the tool already stores. The WebSocket runs on its own thread and
  feeds a `vt100` terminal emulator that is painted into the pane; keystrokes are encoded
  (xterm sequences) and sent back over the socket, and the shell is resized to match the
  pane both ways.

  The protocol was not guessed: it was pinned by reading the running server's handler and
  proven with a live round-trip, and there is an (ignored) integration test that drives
  the real Rust path — `ws_url` + the session thread + the vt100 parser — against a live
  container and asserts a command's output comes back.

## [0.17.0] — 2026-07-18

### Added

- **Cross-service log search — grep every service's logs at once.** Press `g` on
  Services, type a query, and it searches the logs of *all* services in parallel and
  shows the matches grouped by service, newest first. Nothing else in the EasyPanel
  ecosystem does this from a terminal: to find where an error is happening across dozens
  of services, you no longer open them one by one.

  It works because EasyPanel's logs are backed by Grafana Loki (confirmed on the host)
  and `queryServiceLogs` accepts a `search` parameter that runs server-side. The tool
  fans out one request per service on its own thread; measured against the live server,
  searching **"Error" across 33 services took 0.5 s** and pinpointed the three that were
  actually erroring. This is the first feature that makes the tool the place you look
  *first* when something breaks, not a nicer read-only panel.

## [0.16.0] — 2026-07-18

### Added

- **Expose a port from the TUI.** `P` on a service opens a small form (Published, Target,
  Protocol tcp/udp) and creates the port via `ports/createPort`. The TUI could only
  *view* ports (`p`); now it can add them, like the CLI's `port-add` already does. Ports
  are numbers in the API, so the form parses them and rejects non-numbers rather than
  sending a `0` that would open the wrong port. Verified against a live server: the port
  lands in `listPorts`, and `deletePort` (by index) removes it — delete from the TUI and
  a Ports step in the create wizard are the next slice.

## [0.15.0] — 2026-07-18

### Fixed

- **The Status column now means "running", not just "enabled".** It read the API's
  `enabled` field, which only says a service isn't *disabled* — so a crashed or
  never-deployed service still showed **aktif**, a confident lie. Verified against a live
  server: `enabled` is `true` for every service here whether or not it runs; the real
  running signal is whether the service has metrics in `getAllServicesStats` (which only
  lists running containers). Status now has three states — **aktif** (running),
  **berhenti** (enabled but not running: crashed, stopped, or never deployed), **mati**
  (disabled by the user). Before metrics have loaded it falls back to enabled, so it
  never flashes "berhenti" for everything on startup.

## [0.14.1] — 2026-07-18

### Fixed

- **Status messages now fade.** A one-off notification like "Deploy … dimulai" used to
  sit in the status bar forever, because nothing ever cleared it — it only changed when
  the next action wrote over it, which read as "still happening". A message now reverts
  to "Siap" after six idle seconds. The fade is tracked in one place (the event loop),
  not sprinkled across every `self.status =`, and the periodic metric poll deliberately
  never touches the status, so it doesn't keep resetting the timer.

### Changed

- The published crate no longer carries the 0.7 MB `easypanel-api.json` (a developer
  reference the code never reads), `install.sh`, or repo/CI metadata — `cargo package`
  drops from 34 files / 1.2 MB to 23 / 442 KB. No effect on the shipped binary; this is
  `cargo install`/`cargo publish` hygiene. `cargo publish --dry-run` builds the packaged
  crate standalone, and the name is free on crates.io.

## [0.14.0] — 2026-07-18

### Added

- **Creating a service is now a wizard that follows EasyPanel's own flow.** `n` steps
  through **Dasar → Source → Build → Environment → Domains** — `Enter` advances, `Esc`
  goes back, and the title shows where you are (`2/5 Source`). Databases stay a single
  step, because they have no source/build/env/domain to configure. Everything is
  collected once and the service is created in one go — no more create-then-edit.

  This grew from the earlier one-form create, which had become too crowded: the panel
  itself is stepwise, so the CLI now matches it. Any form whose fields all sit on step 0
  still renders as a single page, so nothing else changed.

- **The build engine, environment, and a first domain are part of creation.** Pick
  nixpacks/railpack/dockerfile/buildpacks and its version, paste env vars (opened in
  `$EDITOR`, like the env editor), and set one domain (host, port, HTTPS, path) — all
  before the service exists. `createService` takes build/env/domains inline.

- README badges (CI, latest release, licence) and GitHub issue/PR templates.

- **"Create env file" (dotEnvPath).** The Environment step has a *Buat file .env* toggle
  matching the dashboard; when on, a path field appears (default `.env`) and the env is
  written as a file in the container.

### Fixed

- **Creating a service no longer deploys it immediately.** This was the real cause of
  the "it errored / it never appeared" reports: `createService` with a source inline
  triggers a build-and-deploy that takes ~100 seconds and can fail on a repo that isn't
  ready — all while the row is absent from the table. Measured against a live server,
  the source is what triggers it. So the service is now created **without** a source
  (0.2 s, appears instantly), and the source is applied by a separate `updateSource*`
  call (~2 s, config only, no deploy). Deploy is left as the explicit `d` you press when
  the service is in the table and you're ready — exactly the dashboard's order.

- **Editing a source or build now updates the table.** Changing a branch (`U`) saved
  correctly on the server but the Source column kept showing the old branch until a
  manual `r` — `updateSource`/`updateBuild` returned without asking for a refresh. They
  now refresh the list, same as create and destroy already do.

- **Deploy is dispatched, not awaited.** `d` used to wait for `deployService` to return,
  but a deploy *is* a build — it takes however long the build takes (measured at 125 s
  on one repo, past every proxy's limit; a proxy returned `524` while the build kept
  going). Waiting turned a working deploy into `error sending request`. Deploy now fires
  on a background thread and the status immediately says it started — the build runs on
  the server regardless (dropping the connection doesn't cancel it), and you watch it in
  the logs. Build time varies by repo, so there is no timeout to tune; not waiting is
  the fix.

- **Result and error messages are readable again.** The status bar drew the keybindings
  first and the message last, so a long error was pushed off the right edge and
  truncated (`…ke rep`). The message now leads on the left, in red for errors, and the
  keybindings yield the space when it's tight — they're only a reminder, and `?` has the
  full list. The create status also no longer flashes to a generic "Mengirim…" over its
  own message.

## [0.13.0] — 2026-07-18

### Added

- **`--json` for read-only commands.** Add `--json` to `project list`/`inspect`,
  `stats`, `node list`, `monitor services`/`storage`, `domain list`, `service`
  `ports`/`mounts`/`domains`/`databases`/`backups`/`volume-backups`, `action list`,
  `certificate list` or `notification list`, and it prints the response as JSON instead
  of a table — so the CLI can be scripted, not just read.

  It prints **EasyPanel's own JSON, verbatim**, rather than a shape this tool defines.
  That is deliberate: a hand-rolled schema drifts from the API the moment the server
  changes a field, whereas passing the raw response through cannot. An empty result
  comes out as `[]`, not the human-readable "No X." line, because `[]` is what a
  pipeline into `jq` expects. Verified against a live server across the list commands.

  Implemented as one process-level output flag in `output.rs`, read where each command
  already holds the raw API value — rather than threading a `json: bool` through some
  sixteen function signatures and their call sites for what is really one global choice.

## [0.12.1] — 2026-07-17

### Fixed

- **`server list` could crash on a hand-edited token.** The token column is masked to
  `715b0c…0c72`, and the masking sliced the string by **byte** index (`&token[..6]`).
  The guard that skips short tokens counts bytes too (`len() <= 10`), so it does not
  protect a token that is long enough in bytes but has a multibyte character straddling
  byte 6 — a config file is user-editable, and such a token would panic the command
  outright. Masking now works per character. Proven with a test that panicked before the
  fix (`aaaaa€aaaaa`, `你好世界一二三四五六七`).

### Changed

- **Hardened the two remaining `unwrap`s reachable at runtime.** `confirm_key` and the
  server picker's renderer relied on their callers having checked `is_some()` first;
  both are now total (they do nothing / render without a selection if that ever stops
  holding) rather than panicking. No behaviour change today — this is defence against a
  future caller. The rest of the codebase's `unwrap`s are in tests or provably cannot
  fire; this completes the audit of every one reachable from API responses, config
  files, `$EDITOR`, or terminal size.

## [0.12.0] — 2026-07-17

### Added

- **Dockerfile sources.** The panel offers five source types; this offered three, then
  four. `dockerfile` now works in both the source form (`U`) and the create form (`n`).

  `updateSourceDockerfile` takes the Dockerfile's **contents**, not a path — so it is
  multi-line, and a single-line form field would have been a lie that sends one long
  line that never builds. The content opens in `$VISUAL`/`$EDITOR` with `Space`, reusing
  the hand-off `E` already uses for env; the field itself shows `12 baris` or `(kosong)`,
  because with hundreds of lines what you need to know is whether it is filled in, not
  what its first line says.

  Verified against a live server on a throwaway project, then cleaned up: `createService`
  stored the inline Dockerfile byte-for-byte, newlines intact, and `updateSourceDockerfile`
  persisted an edit. It answers `200` with a body of `{}` — no `json` key — which the
  client already reads as success rather than an error.

### Fixed

- **A Dockerfile source would have been sent labelled `type: "image"`.** `create_source`
  mapped the source type with a `_ => "image"` catch-all, so a fourth type fell through
  it silently. The body would have passed validation and the service would have been
  built from an image nobody named. The mapping is now exhaustive and returns an error
  on anything unknown, and a test fails if the catch-all comes back.

- **The build form's `Dockerfile` field is now `Dockerfile path`.** It holds the path to
  a Dockerfile *in the repository*, while the new source field holds a Dockerfile's
  *contents*. Two fields, same name, opposite meanings — one of them had to say what it
  is.

## [0.11.1] — 2026-07-17

### Changed

- **`src/tui.rs` is now a `tui/` module.** One 5,087-line file held the worker, the
  state, every key handler, every form, and every renderer. It was asked for by the
  project owner in plain terms — hard for a human to maintain — and it was making each
  new feature more expensive than the last.

  The cut follows the data flow rather than the types: `worker` talks to the network on
  another thread and knows only `Req`/`Resp`; `app` holds state and selectors; `keys` is
  a second `impl App` that maps key to action; `render` draws and decides nothing;
  `form` and `table` are the shared vocabulary between them; `mod` keeps the event loop,
  the `$EDITOR` hand-off, and `ServerConfig` — which nothing else may touch. Largest
  file is now ~960 lines.

  **No behaviour change**, and that is the whole point of the release note: the same 83
  tests, not one of them edited, and the test-name list diffed identical before and
  after. The binary was then driven for real — the log viewer, the help overlay and the
  create form all behave as before. Tests stayed in a single `tui/tests.rs` on purpose:
  an untouched suite is what makes it evidence.

## [0.11.0] — 2026-07-17

### Added

- **Service logs tail live.** `Enter` on a service used to fetch 200 lines once and show
  that snapshot forever — open it, and it was already out of date. The pane now sticks
  to the newest line and new output appears as the service produces it. Scrolling up
  pauses the follow (the title says so, rather than leaving you guessing) and `End`
  resumes it.

  There is no streaming endpoint to use: the entire API has one `text/event-stream`
  route and it belongs to the Actions list. So the tail polls `queryServiceLogs` every
  two seconds with `start` set past the newest line already shown — fetching only what
  is new instead of re-pulling 200 lines each round — on the metrics lane, so it never
  queues behind a keystroke. `start` must be a **string** of nanoseconds; a number is
  rejected with "Input validation failed" (established against a live server, not
  guessed). The buffer is capped at 5,000 lines so an hours-long tail cannot grow
  without bound.

  Known limit, stated rather than hidden: two log lines written in the same nanosecond
  would cost the second one. The alternative — an inclusive cursor plus de-duplication —
  re-fetches lines every round to defend against something that does not happen in
  practice.

## [0.10.0] — 2026-07-17

### Added

- **Create an app and its source in one form.** `n` on Services now offers the GitHub
  repo, branch, auto deploy and path alongside the name and type, and sends them with
  `createService` in a single request. Leave the repo empty and the service is created
  bare, exactly as before.

  Create-then-edit was never an API limitation — it was this form's. `createService`
  accepts `source` (and `build`, `env`, `domains`, `ports`, `resources`) inline; only
  `projectName` and `serviceName` are required. Two things in the form machinery stood
  in the way, and both are now gone: a form could only have **one** field controlling
  visibility (this needs two — service type *and* source type), and the create and
  source forms shared the labels `Tipe`, `Image` and `Password`, which `by_label()`
  resolves with `find()` — the merged form would silently have read the wrong field.
  Source labels are now `Source`, `Docker image`, `Registry user`, `Registry password`.

### Fixed

- **A destroyed service stayed in the table.** `destroy`, `start`, `stop` and `restart`
  reported success and never reloaded anything, so a deleted row sat there looking alive
  until you pressed `r`. Same defect class as "a new service doesn't appear", which was
  fixed for create and missed for everything else.

- **Creating an app with a source would have timed out — every time.** Measured against
  a live server: `createService` answers in **0.2 s** without a source and **101 s**
  with a GitHub one, against a 30 s client deadline. The request would have been
  abandoned while the server carried on and created the service anyway, so the TUI would
  report failure and a retry would hit "already exists". Slow calls now get their own
  deadline, and the status line warns before a 1–2 minute wait rather than looking
  frozen. The global timeout stays at 30 s — no other call should have to wait two
  minutes to be told it failed.

## [0.9.0] — 2026-07-17

### Added

- **Auto deploy is visible and switchable from the Services table.** A new **Auto**
  column shows `✓` on, `✗` off, `-` not applicable, and `A` toggles it on the selected
  service.

  The column has three states rather than two on purpose. Auto deploy exists only for
  GitHub sources — it is implemented as a repository webhook — so a MySQL service or an
  image-sourced app has nothing to switch. `✗` there would report "not yet", about
  something that was never possible.

  Verified against a live server, which is the only place this could be learned:
  `enableGithubDeploy` **fails on repositories you do not control**, because creating
  the webhook needs admin rights. EasyPanel forwards that as a `400` wrapping GitHub's
  `404 … /hooks`, and this release names that cause instead of printing the status
  stack. Unrecognised errors are still shown verbatim rather than flattened to "failed".

### Fixed

- **`-0.0 %` CPU on empty projects — for real this time.** v0.7.0 claimed this was
  fixed. It was not: the guard never existed in `render_projects`, and the test that
  "proved" it asserted `vec![].sum()` and `metric_cols(None)` — Rust's float semantics
  and a function the project header row never calls. It passed on every run while the
  screen kept showing `-0.0 %`, which is exactly the failure this project's own agent
  brief warns about: a test that encodes a wrong assumption converts a bug into
  evidence of correctness. The aggregation now lives in `project_row()`, the test calls
  it, and the test fails if the guard is removed.

## [0.8.2] — 2026-07-17

### Fixed

- **A form focused on a dropdown could not be saved.** `Enter` on a choice field
  opened the dropdown; picking a value closed it and left the focus where it was.
  Press `Enter` again and the dropdown just reopened. On "Service baru" with type
  `app`, "Tipe" is the *last* visible field — so the form had no reachable way to
  save at all, and creating an app service was impossible without knowing to `Tab`
  back to a text field first. `Enter` now always saves; `Space` / `←` / `→` open the
  dropdown and toggle yes/no fields.

  The key hints said `[Enter] pilih/simpan` — an honest description of a key that
  was, on a choice field, only ever "pilih". They now read `[Spasi] pilih ·
  [Enter] simpan`, and a focused field spells out which key it wants.

## [0.8.1] — 2026-07-17

### Fixed

- **The builder version could not be changed.** `nixpacksVersion` and
  `railpackVersion` were carefully *preserved* from the original build — and never
  offered for editing, so you were pinned to whatever version the service happened
  to be created with. The panel offers it; now so does the build form.

  Getting there exposed a trap worth naming: the first attempt used two fields both
  labelled "Version", one per builder. `by_label()` uses `find()` — it returns the
  first field with that label, **not the visible one** — so railpack would have
  written nixpacks's version. A comment in this file claimed `by_label` "reads the
  visible field". It does not, and never did; shared labels only worked because each
  was a single field serving both builders. One field now serves both, and a test
  asserts no form has duplicate labels.

## [0.8.0] — 2026-07-17

### Security

- **Upgraded ratatui 0.29 → 0.30**, which clears the only advisory that actually
  reached users. ratatui 0.29 shipped `lru` 0.12, unsound per RUSTSEC-2026-0002
  (`IterMut` violates Stacked Borrows); 0.30 uses `lru` 0.18 and drops `paste`
  entirely. `cargo audit` goes from 3 warnings to 1, and the remaining one
  (`async-std`) arrives via `httpmock`, a dev-dependency that never ships.

  crossterm came along from 0.28 to 0.29 as a transitive bump — that is the key
  handling, so every screen was exercised rather than assumed: dashboard gauges and
  sparklines, hosts, maintenance, actions, monitor, domains, the services
  hierarchy, and the help overlay all render, with no panics.

  Required no code changes. A major bump that touches nothing is worth stating
  plainly rather than dressing up as a migration.

## [0.7.0] — 2026-07-17

### Added

- **Database services ask for what the panel asks for.** Creating mysql, mariadb,
  postgres, mongo or redis now offers database name, user, password, root password
  and image, with the fields swapping to match the type — redis has no database
  name, only mysql and mariadb have a root password. Previously only the name and
  type were sent, so every database service came out with server-generated
  credentials you never saw.

  All fields are optional, and an empty one is **omitted** from the request rather
  than sent as `""`. Those are not the same thing, and the difference is not a
  matter of taste — measured against a live server:

  | | `databaseName` | `user` | `password` |
  |---|---|---|---|
  | field omitted | `zzz-dbtest` (project name) | `mysql` | generated, 20 chars |
  | field sent as `""` | `None` | `None` | `None` |

  Sending empty strings produces a MySQL with no database, no user and no password.

### Fixed

- **A project with no services showed `-0.0 %` CPU.** Rust's `Sum` for `f64` uses
  `-0.0` as its identity (so that `-0.0 + x` preserves the sign of `x`), which means
  summing zero values yields `-0.0`, and `{:.1}` prints it verbatim. A negative CPU
  reading is a confidently wrong number. Projects with no services now show `-`,
  because nothing was measured — not `0`, which would claim it was.

## [0.6.0] — 2026-07-17

### Added

- **`cargo audit` in CI**, on every push and weekly on a schedule — advisories
  appear whether or not you commit, so a push-only check would miss them until the
  next unrelated change. Reports 0 vulnerabilities today. Deliberately *without*
  `--deny warnings`: that would paint CI red until ratatui 0.30 lands, and a CI
  that is always red stops being read.

### Changed

- **Services shows the hierarchy again — in one table.** A project header with its
  service count and aggregate metrics, followed by its services. The flat list
  fixed drill-down but broke something else: a table of *services* has no row for a
  project with none, so creating a project appeared to do nothing, and the project
  could not be selected, filled, or deleted. Selecting a header now targets the
  project (`n` new service, `X` destroy); selecting a service targets the service.
  A header can never be mistaken for a service — service actions refuse it.

## [0.5.2] — 2026-07-17

### Fixed

- **You could get locked into the current branch.** The Branch field is a dropdown
  filled from GitHub via the panel. When that list fails to load — a revoked GitHub
  token in EasyPanel does it — the dropdown was left holding only the current value,
  so the branch could not be changed at all. A one-option dropdown is a locked door,
  not graceful degradation. It now falls back to a text field; the server validates
  the branch either way (it rejects unknown ones with "Branch not found").
- **The error said nothing useful.** EasyPanel wraps upstream failures, so a dead
  GitHub token surfaced as `[400] Request failed with status code 403 Forbidden` —
  two status codes and no hint about what to fix. It now names the cause and points
  at the GitHub token in EasyPanel's settings.

## [0.5.1] — 2026-07-17

### Fixed

- **A corrupt `servers.json` could silently delete every server.** `all()` turned
  any read or parse failure into "no servers exist", and `add`, `remove` and
  `set_default` all read through it and then save the result — so one unreadable
  config plus one command would rewrite the file from scratch, taking every token
  with it. Tokens cannot be read back from anywhere, so the loss is permanent.

  Write paths now refuse to save when the config can't be read, and say what to do.
  A *missing* file still means "no servers" — that's a first run, not damage. Read
  paths stay soft: a corrupt file shows an empty list rather than panicking and
  leaving the terminal in raw mode with the TUI open.

## [0.5.0] — 2026-07-17

### Changed

- **The CLI now speaks English.** `--help`, the man page, shell completions, table
  headers, confirmations, error messages and results are all translated — they come
  from one source, so translating the help text moved all three surfaces at once.
  The README promised English while the first thing a new user saw was
  `Belum ada server. Jalankan: easypanel server add`. Half-translating would have
  been worse than not translating: it reads as broken rather than foreign.

  Code comments and commit messages stay in Indonesian, as before. The TUI is not
  translated yet.

## [0.4.0] — 2026-07-17

### Added

- **Man page** — `easypanel man` prints roff to stdout. Release tarballs now ship
  `easypanel.1` next to the binary, and `install.sh` installs it into the first
  writable `man1` directory it finds. Verified by actually rendering it with `man`,
  not by counting bytes: NAME, SYNOPSIS, DESCRIPTION, OPTIONS, SUBCOMMANDS and
  VERSION all appear.

### Fixed

- **Release packaging could have shipped an empty man page.** `> easypanel.1`
  creates the file *before* the binary runs, so a cross-compiled target that cannot
  execute would still leave a zero-byte file behind, and a `-f` check would happily
  pack it. Now checked with `-s`, and skipped loudly when empty.

## [0.3.0] — 2026-07-17

### Added

- **Shell completions** for bash, zsh, fish, elvish and PowerShell via
  `easypanel completions <shell>`. With no argument the shell is guessed from
  `$SHELL`. `install.sh` now installs zsh/bash/fish completions automatically —
  but only into directories that already exist, since creating completion
  directories on someone else's system isn't an installer's business.
- Two tests that keep it honest: clap's `debug_assert()` validates the whole CLI
  definition (duplicate names, conflicting args) at test time instead of at
  runtime, and every shell's script is checked for real subcommands — a generator
  that emits an empty-but-valid script would otherwise pass silently.

## [0.2.0] — 2026-07-16

The release that makes this project legally usable and legible to people who
didn't write it.

### Added

- **MIT licence.** Without one, nobody could legally use this code — the single
  biggest barrier to adoption.
- `CONTRIBUTING.md` and `.github/AGENT_BRIEF.md` documenting how this project is
  built and the failure modes it keeps hitting.
- Crate metadata (licence, repository, keywords, categories) for publishing.

### Changed

- README is now in English for reach; code comments and commit messages stay in
  Indonesian.

## [0.1.0] — 2026-07-16

First release. Rust rewrite of an earlier PHP prototype.

### Added

- **Multi-host management.** Credentials in `~/.config/easypanel/servers.json`
  (`0600`); every command takes `--server`.
- **Full-screen TUI** (ratatui): Dashboard, Hosts, Maintenance, Actions, Monitor,
  Domains, Services.
  - **Hosts** stacks *every* configured server at once — CPU, memory, disk, load per
    host, each fetched on its own thread so a slow or dead host never blocks the
    others. Failed hosts show red with the reason instead of failing the table. This
    is the one screen the web panel cannot replace.
  - **Services** is a flat, searchable table of every service across every project,
    with source (`owner/repo#branch`) and live CPU/memory/network per service.
  - `/` filters Services, Domains, Actions, and Monitor.
  - `?` lists every shortcut for the current screen.
- **Source & build config** for app services — GitHub/git/image sources with repo and
  branch dropdowns fed by live GitHub data; nixpacks/railpack/dockerfile/buildpacks.
- **Domains** — create/edit/delete, SSL resolver, wildcard, service or custom
  destinations with weights.
- Env editing in `$EDITOR`, ports, mounts, backups, certificates, notifications,
  cluster nodes, database restore, Docker maintenance.
- Binaries for linux-x86_64, darwin-arm64, darwin-x86_64.

### Fixed

Bugs found by testing against a real server — none of these were visible to unit
tests:

- **`updateSourceGithub` silently disables auto-deploy.** The endpoint resets
  `autoDeploy` to `false` on every successful call, so merely changing a branch would
  turn off auto-deploy on a production service. The TUI now exposes an explicit
  toggle and restores the value afterwards.
- **Every server error message was discarded.** EasyPanel nests `message` inside
  `json`; the client read the top level, so `Branch not found` and
  `Project not found.` both surfaced as `[400] Bad Request`. The existing test passed
  because its mock used a shape the server never sends.
- **Domain edits destroyed data.** Editing rebuilt the body from scratch, dropping
  middlewares, certificate resolvers, wildcard flags, and every custom server after
  the first.
- **Load average always read `0.00`.** `loadAvg` is a flat array of three strings, not
  a `[timestamp, value]` series, so the series reader silently returned zero.
- **Dropdowns silently changed config.** A value missing from a freshly loaded option
  list jumped to the first option — enough to change the deployed branch just by
  opening a form.
- **Deleting a server needed no confirmation**, discarding a token that cannot be read
  back from anywhere.
- **`E` failed with "No such file or directory"** when `$EDITOR` pointed at an editor
  that was not installed; the message read as if the env file were missing.
- **`Esc` quit the whole TUI** instead of cancelling.
- Unreadable status bar and sparklines (named colours are reinterpreted by terminal
  themes; palette indices are not).

[Unreleased]: https://github.com/mrfansi/easypanel-cli/compare/v0.8.1...HEAD
[0.8.1]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.8.1
[0.8.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.8.0
[0.7.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.7.0
[0.6.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.6.0
[0.5.2]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.5.2
[0.5.1]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.5.1
[0.5.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.5.0
[0.4.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.4.0
[0.3.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.3.0
[0.2.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.2.0
[0.1.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.1.0
