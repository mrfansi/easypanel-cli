# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.98.8] — 2026-07-30

### Added

- Cloudflare Workers version history now has a `Status` column, so a list of
  rows all reading `100%` no longer hides which deployment is serving traffic:
  `live` for the active one, `rolling out` while a gradual deployment still
  splits traffic across versions, `superseded` for everything below it. The
  active-deployment pane and `easypanel cf workers deployments` (table and
  `--json`) report the same status.

  Cloudflare only lists deployments that already exist, so there is no
  "uploading" state to show — an unfinished deployment is a rollout that has
  not landed on one version at 100%.

## [0.98.7] — 2026-07-26

### Added

- EasyPanel Domains now supports the same scalable mark/select-all workflow as
  the rest of the TUI: `v` marks one domain, `V` selects every domain currently
  shown by the active filter, and `Space` opens a bulk menu.
- Marked Domains can now be bulk-edited or bulk-deleted from that menu. Bulk
  delete requires typed confirmation (`DELETE N`) before sending any destructive
  request.

## [0.98.6] — 2026-07-26

### Fixed

- EasyPanel Domains filters now match the raw hostname in addition to the
  rendered `https://host/path` source text, so anchored regex filters such as
  `^[^.]+\.viding\.co$` work as operators expect.

## [0.98.5] — 2026-07-25

### Changed

- Cloudflare DNS bulk patch/delete now use Cloudflare's DNS batch endpoint instead
  of one API request per record. Requests are chunked at 200 records per batch, the
  documented Free-plan limit, so the same flow is safer across every Cloudflare plan
  and less likely to run into per-record API rate-limit storms.
- The batched DNS path is shared by the TUI bulk DNS actions, CLI
  `cf record set`, and CLI multi-id `cf record delete`.

### Fixed

- Large TUI DNS bulk deletes now require typed confirmation (`DELETE N`) when more
  than 100 records are selected, reducing the chance of accidentally deleting a
  broad production filter result.

## [0.98.4] — 2026-07-25

### Changed

- TUI filters are now regex-capable across EasyPanel and Cloudflare tables while
  keeping the old case-insensitive substring behavior for ordinary text.
- Bulk selection is now virtual and scalable: **V** selects all rows currently shown
  by the active filter, or the whole current list when no filter is applied, without
  materializing thousands of marks.

### Fixed

- Bulk actions now execute correctly from select-all mode on Services, Cloudflare
  DNS records, and R2 objects; confirmation and dispatch resolve the final targets
  from the same filtered list the user sees.

## [0.98.3] — 2026-07-25

### Added

- Cloudflare Tunnels now has full tunnel lifecycle coverage in CLI and TUI:
  `cf tunnels create`, `cf tunnels install`, and `cf tunnels delete`, plus TUI
  shortcuts **i** for install commands and **x** for typed-name deletion.
- Tunnel route add/edit/delete forms in the TUI now include DNS CNAME sync fields,
  so published application routes can update or delete the matching
  `<tunnel-id>.cfargotunnel.com` CNAME without leaving the route workflow.

### Changed

- Tunnel install output opens in the TUI viewer and CLI install output prints both
  Linux service and Docker commands, with the connector token shown only for that
  explicit install flow.

## [0.98.2] — 2026-07-24

### Added

- Cloudflare Tunnels can now be created from the TUI. On the Tunnels list, **n**
  opens a remotely configured tunnel form (`config_src=cloudflare`), and the
  Space menu / command palette expose the same create action.

### Changed

- Tunnel route add/edit forms now mirror Cloudflare's published-application flow:
  **Service Type** is a choice (`http`, `https`, `unix`, `unix+tls`, `tcp`, `ssh`,
  `rdp`, `smb`, `http_status`, `bastion`, `hello_world`) and **Service URL** is
  collected separately where that type needs one.
- Removed **Advanced origin JSON** from Tunnel route forms. Origin request options
  are now explicit fields for TLS, HTTP, connection, proxy, keep-alive, and Access
  JWT validation settings, while unknown existing keys are preserved on edit.

### Fixed

- Tunnel service validation now accepts Cloudflare's documented `unix:/path`,
  `unix+tls:/path`, `bastion`, and `hello_world` service values.

## [0.98.1] — 2026-07-24

### Changed

- Tunnel route forms now show `noTLSVerify` as a **No TLS verify** toggle instead of
  forcing operators to edit raw `{"noTLSVerify":true}` JSON. Less common
  `originRequest` keys remain available through **Advanced origin JSON**.

## [0.98.0] — 2026-07-24

### Added

- Cloudflare Tunnels can now manage published application routes. The CLI adds
  `easypanel cf tunnels route add|edit|delete`, performs Cloudflare's required
  read-modify-write on `config.ingress`, preserves the catch-all rule at the end,
  validates Cloudflare service prefixes, and can create/update/delete the matching
  public CNAME via `--dns` / `--delete-dns`.
- The TUI Tunnel config screen is no longer read-only: **n** adds a route, **e**
  edits the selected route, **x** deletes it with typed-hostname confirmation, and
  `Space` / right-click opens the route action menu.
- Added route-mutation unit coverage for catch-all ordering, duplicate rejection,
  service validation, edit, and delete flows.

## [0.97.0] — 2026-07-24

### Added

- Cloudflare Tunnels is now a product tab between Domains and R2. The TUI lists
  account-scoped cloudflared tunnels with status, config source, created date,
  target, and id; **Enter** opens the selected tunnel's published
  routes/configuration table.
- Added CLI Tunnels views:
  `easypanel cf tunnels list` and `easypanel cf tunnels config <tunnel>`, with
  normal table output and JSON output support.

### Changed

- Cloudflare product shortcuts are now `1` Analytics, `2` Domains, `3` Tunnels,
  `4` R2, and `5` Workers.
- Cloudflare permission hints now call out missing **Cloudflare Tunnel Read** /
  **Cloudflare One Connectors Read** access for Tunnels errors.

## [0.96.0] — 2026-07-24

### Added

- Cloudflare Workers now has a per-Worker settings/configuration drill-in. Press
  **s** on a Worker (or choose **View settings** from the row menu / command
  palette) to inspect variables and bindings, secret metadata, cron triggers,
  observability, runtime compatibility, limits/cache/placement, and general
  Worker metadata. From the settings view, **d** jumps back to deployments.
- Added `easypanel cf workers settings <name>` for a CLI view of the same Worker
  configuration bundle, with normal table output and JSON output support.

### Fixed

- Worker settings parsing now accepts Cloudflare's real-world `null` arrays for
  fields such as bindings, compatibility flags, tags, tail consumers, and cron
  schedules.

## [0.95.0] — 2026-07-24

### Added

- Cloudflare Workers now has a per-Worker deployments/version-history drill-in.
  Press **Enter** on a Worker (or choose **View deployments** from the row menu /
  command palette) to see the active deployment summary and the full deployment
  history with versions, traffic percentage, source, trigger, author, timestamp,
  and deployment message.
- Added `easypanel cf workers deployments <name>` for a read-only CLI view of the
  same Cloudflare Workers deployments API, with normal table output and JSON
  output support.

## [0.94.0] — 2026-07-24

### Added

- Cloudflare Workers is now a full account-scoped product tab beside Analytics,
  Domains, and R2. The TUI lists Worker scripts with handlers, usage model,
  modified date, and etag; supports `/` filtering, `n` deploy/replace from a
  local file, `x` typed-name delete, row menus, right-click actions, command
  palette actions, and `1-4`/Tab product switching.
- Added CLI Workers management:
  `easypanel cf workers list|get|deploy|delete`. Deploy uploads one local
  JavaScript file through Cloudflare's Workers Scripts content endpoint and
  supports both modern module syntax (`--mode module`) and legacy service-worker
  syntax (`--mode service-worker`).

### Changed

- Cloudflare permission hints now call out missing **Workers Scripts** access for
  Workers list/get/deploy/delete errors.

## [0.93.5] — 2026-07-23

### Fixed

- Cloudflare envelope parsing now accepts `errors: null` / `messages: null`
  responses, fixing the Web Analytics metadata fetch that previously surfaced
  `invalid type: null, expected a sequence` in the Domains status bar.

## [0.93.4] — 2026-07-23

### Fixed

- Cloudflare Domains no longer treats a Web Analytics metadata permission failure
  as a fatal Domains error. Zones remain visible, Web Analytics columns stay `-`,
  and the status line now hints that `Account Settings Read` is needed.

## [0.93.3] — 2026-07-23

### Added

- Cloudflare Domains now enriches the zones listing with Web Analytics metadata
  from Cloudflare's RUM Site Info API, showing whether each domain has Web
  Analytics enabled, its setup mode, created date, and optional 24-hour
  page-view/visit columns when Cloudflare returns those totals.

## [0.93.2] — 2026-07-23

### Fixed

- Cloudflare Analytics now renders numeric GraphQL dimensions such as
  `edgeResponseStatus` as real status codes instead of `-`.
- Cloudflare Analytics expands common country codes like `ID`, `US`, and `SG`
  into dashboard-style country names in the Top countries table.

## [0.93.1] — 2026-07-23

### Fixed

- Cloudflare Analytics no longer asks GraphQL for the account-level
  `edgeResponseContentTypeName` dimension. Some real accounts reject that field,
  which made the whole Analytics tab fail before any stable metrics could render.

## [0.93.0] — 2026-07-23

### Added

- Cloudflare: account-level Analytics is now product tab `1`, before Domains. It
  renders a terminal dashboard for requests, bandwidth, visits, top countries,
  SSL/cache/status/protocol breakdowns, using Cloudflare's GraphQL
  analytics API and the active account's `account_id`.

### Changed

- Cloudflare: product shortcuts are now `1` Analytics, `2` Domains, `3` R2. Domains remains
  the workspace landing screen so existing DNS/R2 muscle memory is not broken, but
  the tab bar now matches Cloudflare's account dashboard order with Analytics first.

## [0.92.0] — 2026-07-23

### Added

- Cloudflare: the command palette now starts with actions for the selected row
  before the navigation entries. Zones can open records or delete, DNS records
  can edit/delete, R2 buckets can browse/delete, and R2 object levels can upload,
  download, or delete from the same `:` flow Easypanel users already reach for.
- Cloudflare: account picker now supports editing an existing account (`e`) so a
  token or account-id can be fixed without deleting and recreating the entry.
- Cloudflare: DNS/R2 product tabs are clickable, matching the mouse behaviour of
  the Easypanel tab bar.

### Changed

- Cloudflare: the workspace chrome now gets a stable tint per active account,
  while keeping Cloudflare orange for Cloudflare-specific state such as proxied
  DNS records. This makes switching Cloudflare accounts feel closer to switching
  Easypanel hosts: the context changes in colour as well as text.
- Cloudflare: DNS Records now uses `Space` for the selected row's action menu
  until records are marked, then switches to the bulk menu. That removes the old
  dead-feeling path where pressing `Space` on an unmarked record only complained
  about missing marks.
- Cloudflare: status hints now advertise the `:` palette and call out
  `Space menu/bulk`, so the richer interaction model is discoverable without
  opening help.

### Fixed

- Cloudflare: non-error feedback such as "No record selected" or "Name did not
  match — nothing deleted" now stays visible in the status bar instead of being
  hidden behind resting key hints.
- Cloudflare: switching accounts while on R2 no longer leaves stale DNS zones
  visible when returning to DNS; product switches reload the active account's
  home data.
- Cloudflare: CF lists no longer show a fake loading state just because an
  unrelated Easypanel request is busy.
- Cloudflare: DNS record and bulk forms now reject invalid TTL/priority values
  instead of silently treating them as automatic/default values.
- Cloudflare: bulk DNS failures now open a readable report viewer with per-record
  details instead of cramming the whole failure list into one status line.

## [0.91.0] — 2026-07-23

### Changed

- Cloudflare: the list titles and the filter prompt now use EasyPanel's exact
  grammar, sharing its `count_title` helper instead of imitating it. At rest a
  title reads ` Zones (47) ` (just the total, padded like every EasyPanel
  title); while filtering it reads ` Zones (18/47)  /ed▏ ` and keeps the
  applied filter named — so the two workspaces read as one app, and the title
  format can no longer drift because both are built by the same function.
- Cloudflare: the filter prompt in the status bar is now the same widget
  EasyPanel uses, gaining the `↑↓ select` hint — the arrows really did move
  the selection while typing, but nothing said so.

## [0.90.0] — 2026-07-23

### Added

- Cloudflare: marking rows now gives the same feedback EasyPanel does. The
  table title carries the `· ✓ N marked` suffix, and while marks exist the
  status bar shows `N record(s)/file(s) marked — [Space] to act on them,
  [Esc] to clear` instead of the resting key hints — so after marking you can
  see, without scrolling, how many rows a bulk action will hit and how to back
  out. Before this, marks on the DNS Records and R2 Objects screens were only
  visible as small ✓ glyphs far from the cursor.

### Fixed

- Cloudflare: switching product tabs (DNS ↔ R2) now clears pending marks.
  They used to survive the switch invisibly, which would have shown a stale
  "marked" message on a screen whose `Space`/`Esc` do something else entirely.

## [0.89.1] — 2026-07-23

### Fixed

- **`~` in an R2 upload/download path now means your home directory.** The path typed
  into the TUI upload ("Local file path") and download ("Save to") forms is a raw string
  — a shell isn't there to expand it — so `~/dump.sql.gz` was taken literally and failed
  with a confusing "Can't read ~/dump.sql.gz: No such file", even though the file was
  sitting in your home directory. A leading `~`/`~/` is now expanded to `$HOME` before the
  transfer (a pure `output::expand_tilde`, unit-tested; `~user` is left alone, and an
  unknown `$HOME` leaves the path untouched rather than guessing). Verified live: uploading
  `~/file` from the form now resolves and succeeds. The CLI was already fine (the shell
  expands `~` there).

## [0.89.0] — 2026-07-23

### Added

- **R2 objects: upload, download, delete, and bulk delete/download.** The object browser
  was read-only; now you can manage a bucket's contents end to end, over the same
  Cloudflare REST API (Bearer token, no S3 credentials).
  - **TUI** (R2 → a bucket → objects): `u` uploads a local file to the current folder;
    `Enter` (or the row menu) downloads the selected file, choosing where to save it; `x`
    deletes it behind a typed-nothing confirm that names the **account** and scopes the
    action to the selected object(s); `v`/`V` mark files and `Space` opens a bulk menu —
    **Download N marked** / **Delete N marked** — the same mark-and-bulk flow as DNS
    records. Uploads and deletes reload the folder; the confirm dialog and folder
    rendering keep their recent fixes.
  - **CLI**: `easypanel cf r2 object put <bucket> <key> --file <path>`,
    `get <bucket> <key> [--out <path>]` (refuses to overwrite), and
    `rm <bucket> <key>…` (one or many — the bulk form).
  - **Limits, honestly enforced**: this REST endpoint caps a single upload at **300 MB**
    (larger objects need the S3 multipart API, which this tool uses only for DB dumps) —
    oversize uploads are refused with that reason rather than failing obscurely.
    Downloads stream to disk (no buffering), object keys are correctly percent-encoded
    (slashes kept literal), and the needed token permission is the account-level *Workers
    R2 Storage* (Edit for write). All verified live end to end against a throwaway bucket:
    upload → list → download (byte-identical) → bulk-delete → cleanup.
  - Domain rules (key encoding, the 300 MB guard, key/basename building) live in
    `cloudflare.rs` (pure, unit-tested); the client, CLI, worker and TUI are thin callers.

## [0.88.0] — 2026-07-23

### Changed

- **The Cloudflare lists now colour their state, the way the EasyPanel tables colour
  Status.** Two columns that carry the state an operator scans for were plain, undifferentiated
  text; now they read at a glance, matching EasyPanel's green/red Status convention:
  - **Zone Status** — `active` green, `pending`/`initializing` yellow (nameservers not
    moved yet — you have to act), `moved`/`deactivated`/`deleted` red (not serving). A
    zone that isn't live no longer looks identical to a healthy one. The status
    classification is a pure domain function (`cloudflare::zone_health`, unit-tested), so
    the renderer only maps a category to a colour.
  - **Record Proxied** — proxied records show the flag in **Cloudflare orange** (the
    orange-cloud state: origin hidden, WAF/CDN on), DNS-only records in grey (origin IP
    exposed) — colour *is* how Cloudflare's own dashboard represents this field, and it's
    the fastest way to spot a record that's accidentally exposing the origin.
  Reuses the table renderer's existing per-cell styler; no other columns change.

## [0.87.7] — 2026-07-23

### Fixed

- **Right-clicking a file in the R2 object browser no longer offers to delete the bucket
  you're inside.** The right-click handler opened the bucket action menu for the whole R2
  product, without checking which screen you were on — so on the Objects drill-in,
  right-clicking a file popped up "Browse objects / **Delete bucket…**", a destructive
  action aimed at the very bucket you were browsing (and reachable from a stray
  right-click on a data dump). The Objects screen is browse-only for now, so it opens no
  row menu at all; the bucket menu stays where it belongs — the Buckets home. `Space`
  was already correct (it routes to the objects handler, which has no menu); only the
  mouse path was wrong.

## [0.87.6] — 2026-07-23

### Fixed

- **The selected folder in the R2 object browser is no longer a two-tone bar, and the
  rows are calmer.** Folders were painted in bold CF **orange** across the whole Name
  column. The selected row is drawn reversed (fg↔bg swap), so that full-width orange
  foreground became a full-width orange **background** — beside the reversed default of
  the empty Size/Modified cells, the highlight came out as a jarring two-tone bar
  (orange then lavender). It also meant every folder row was a block of saturated orange,
  the same colour as the workspace chrome, so the screen read as orange-on-orange. Set a
  folder apart with **bold** instead of a colour: the `▸ ` marker and trailing `/`
  already say "folder", bold adds weight, and the selection now reverses to one uniform
  bar. CF orange stays where it carries meaning — the borders, header, and breadcrumb —
  rather than flooding every row. Verified live on a real bucket.

## [0.87.5] — 2026-07-23

### Fixed

- **The Cloudflare Zones status bar now advertises `Space menu`.** `Space` opens the
  zone row action menu (Open DNS records / Delete zone) — but the Zones status-bar hint
  was the only CF list that didn't say so, while its siblings did (`Space menu` on
  Buckets, `Space bulk` on Records). The row menu was there but undiscoverable from the
  one always-visible hint line. The Zones hint now reads `… · x delete · Space menu · /
  filter · …`, matching the Buckets hint. A test pins every Space-bound CF screen to
  advertising it, so the hint can't drift from the keybinding again.

## [0.87.4] — 2026-07-23

### Fixed

- **The Cloudflare account switch `a` now works on every screen, like EasyPanel's
  `s`.** EasyPanel's server switcher `s` sits above the per-screen key dispatch, so it
  works everywhere. The CF account picker `a` was bound only on the Zones and Buckets
  home screens; on the Records and R2 Objects drill-ins it fell through to the table
  navigation and did nothing — a silent dead key, right where you'd want to jump to
  another account after inspecting a zone. `a` is now handled once in the CF dispatch
  (alongside `:` and the product-tab keys), so it opens the account picker from any CF
  screen and returns to the new account's home. Both the `?` help and the status-bar
  hints for Records and Objects now list `a account`, matching Zones and Buckets.
  Verified live: `a` on a zone's DNS records opened the account picker.

## [0.87.3] — 2026-07-23

### Fixed

- **Arrow keys now move the Cloudflare list while you type a filter — as they do in
  EasyPanel.** In the EasyPanel workspace, pressing `/` and typing to narrow a list lets
  you reach straight for ↑/↓ (or PageUp/PageDown/Home/End) to select the row you just
  filtered to, without an intervening Enter — the filter box explicitly keeps the list
  live. The Cloudflare filter did not: those keys were inert while typing, so after
  narrowing zones/records/buckets/objects you had to press Enter to leave the filter
  before the arrows did anything — the selection appeared stuck. `cf_filter_key` now
  routes the movement keys to the active CF table (via the same
  `active_table`/`visible_table_len` the mouse layer uses), so type-then-arrow works
  identically in both workspaces. Verified live: filtering a zone's DNS records and
  pressing ↓ moved the highlight within the narrowed list mid-type.

## [0.87.2] — 2026-07-23

### Fixed

- **A Cloudflare delete confirmation no longer claims it "Affects the ENTIRE host" or
  names the wrong machine.** The confirm dialog is shared with EasyPanel, and it
  hard-coded EasyPanel semantics: it always printed `on <easypanel-server>` and derived
  a target line from the project/service fields. A CF confirm carries neither, so
  deleting DNS records showed the unrelated EasyPanel host name (`on harisenin-angelia`)
  and — because the empty-project case is read as a host-wide "maintenance" action —
  the alarming, false warning **"Affects the ENTIRE host."** Deleting a DNS record
  touches no EasyPanel host at all. A `cf-*` confirm now renders its own body: the
  active Cloudflare **account** in CF orange (the analogue of "which machine", shown for
  the record/bulk deletes that act within it; omitted for account-removal, whose target
  account is already named in the label and may not be the active one) and a scope line
  that is actually true — "Affects only the selected DNS record(s)." or, for account
  removal, "Local config only — your Cloudflare account is untouched." Verified live: a
  bulk record delete now reads `on pt-aku-bisa-ibadah` / "Affects only the selected DNS
  record(s)."

## [0.87.1] — 2026-07-23

### Fixed

- **The Cloudflare status bar now signals "working" and never swallows an action
  error.** EasyPanel's status bar shows a spinner + message while a request is in
  flight ("working, not frozen") and keeps an error visible until you act on it. The CF
  workspace's status bar rendered *only* the resting per-screen key hints — it returned
  before ever consulting the spinner or the status message — so a load, a refresh, or a
  mutation gave no busy feedback in the bar, and an action error (`No zone selected`, or
  a create/delete the API rejected) vanished silently: the list body reports a failed
  *load*, but not a failed *action*. The bar now yields the hints to live feedback,
  exactly like EasyPanel: a spinner + the working message while `busy` is non-zero (the
  same counter EasyPanel's spinner uses), the error message in pink when an action
  fails, and the resting hints only when idle. Verified live: drilling into a zone's
  records showed `⠧ Loading records for …` in the bar, then fell back to the Records
  hints once the load settled.

## [0.87.0] — 2026-07-22

### Added

- **A `:` command palette in the Cloudflare workspace — the mirror of EasyPanel's.**
  EasyPanel's `:` opens a global jump-anywhere search; the CF workspace's isolation gate
  left `:` dead, so the two halves of the same app behaved differently. `:` now opens a
  CF palette that lists — and jumps to — every **product tab** (DNS · R2), every
  configured **account** (the active one flagged), every **zone** (→ its DNS records),
  and every loaded **bucket** (→ its objects). Type to filter, ↑↓ to select, Enter to
  jump, Esc to close — the same overlay, filter, and keys as the EasyPanel palette,
  reusing the same widget rather than a parallel one. Jumping to a zone or bucket first
  switches to the right product tab, so `:` reaches an R2 bucket even from the DNS
  screen. Jumps run **by identity** (a zone's own id, a bucket's name), never a
  filtered-row index a live filter could have shifted, so the palette always opens the
  thing you picked. Verified live against a real account (9 zones): `:` → typed a zone
  name → Enter drilled straight into that zone's 11 DNS records. Documented in the
  workspace help (`?`). Contextual row actions (edit/delete a record from the palette)
  are a later slice; this ships the navigation half.

## [0.86.1] — 2026-07-22

### Fixed

- **Cloudflare workspace: the product tabs (DNS · R2) are now documented in the
  help.** The header has shown a `DNS │ R2` tab bar since R2 landed, and `1`/`2`/`Tab`/
  `←→` have switched between them — but the help overlay (`?`) never listed those keys,
  so the second product was visible yet undiscoverable. A reader who opened `?` to learn
  "how do I move around here" was told about `W`, `?`, `r` and the per-screen keys, but
  nothing about reaching R2. The "Anywhere" section now carries **`1-2 / Tab / ←→
  switch product tab`**, mirroring exactly how the EasyPanel help documents its own
  `1-8 / Tab / ←→ switch tab`. The upper bound is pinned to the product list by a test,
  so the next product (D1, KV, …) can't outrun the hint the way this one did. The stale
  code comments that still claimed the CF tab keys were "inert" (true only when DNS was
  the lone product) were corrected too.

## [0.86.0] — 2026-07-22

### Changed

- **R2 objects now browse as a folder tree, not one flat 1000-row dump.** A bucket whose
  keys look like `assets/admin-front-end/css/foo.css` was showing every object in one long
  list; it now shows one level at a time — the subfolders (as `▸ name/`) and the files
  directly at that level — like Cloudflare's dashboard or any S3 browser. **Enter** descends
  into a folder, **Esc** goes up a level (then back to the buckets list at the root), and
  the breadcrumb shows the current path. Folders are listed A→Z and files **newest-first**;
  `/` still filters the current level. This uses the objects API's `delimiter` mode over the
  same API token. On the CLI, `cf r2 object list <bucket> [--prefix path/]` shows the same
  level view (and its `--json` now returns `{ "folders": […], "files": […] }`).

### Added

- **Right-click a DNS record for its action menu**, like everywhere in EasyPanel. The CF
  Records screen previously ignored right-click (a record's edit/delete were keys-only);
  it now opens a per-record menu (*Edit record* / *Delete record…*), matching the zone and
  bucket row menus. `Space` there still opens the bulk menu for marked records.

### Fixed

- **Browsing a large R2 bucket no longer hangs on "Loading objects…".** The object list
  followed every pagination cursor before showing anything, so a bucket with tens of
  thousands of objects never finished loading (in the TUI it sat on "Loading objects…"
  forever; on the CLI it spun). It now loads the **first page** (up to 1000) and says so —
  the TUI title reads "… · first page, more exist — narrow with /", the CLI prints
  "Showing the first 1000 — narrow with --prefix". Use `/` (TUI) or `--prefix` (CLI) to
  drill into a path.

## [0.85.0] — 2026-07-22

### Added

- **Browse the objects inside an R2 bucket.** In the TUI, **Enter** on a bucket (R2 tab)
  drills into its objects — Key / Size / Modified, with a `/` filter — mirroring the
  zone→records drill-in; on the CLI, `easypanel cf r2 object list <bucket> [--prefix …]`.
  It uses Cloudflare's REST objects API with the account's **existing API token** (the
  same one that lists buckets) — **no separate R2 S3 credentials required**. Verified live
  against real buckets and objects. (Uploading/downloading/deleting objects is still to
  come.)

### Added

- **Cloudflare R2 — manage buckets.** The Cloudflare workspace's product tab bar grew a
  second tab: **R2** beside DNS (switch with `1`/`2`/`Tab`/`←→`). It lists the active
  account's R2 buckets (name, created, location, storage class), and `n` creates a bucket
  while `x` (or the `Space`/right-click row menu) deletes one — the same shape as the DNS
  Zones screen. On the CLI: `easypanel cf r2 bucket list|create|delete`. Uses the account's
  API token (needs the account-scoped **Workers R2 Storage** permission — the tool hints
  at that if the token lacks it, the same way it does for Zone:DNS). Verified live against
  a real account's buckets. (Browsing the *objects* inside a bucket comes next — that goes
  through R2's separate S3 credentials.)

## [0.83.4] — 2026-07-22

### Added

- **The Cloudflare Zones screen now has a row action menu, like the EasyPanel screens.**
  Press **`Space`** (or **right-click** a zone) to open an "Actions" menu — *Open DNS
  records* and *Delete zone…* — the same `open_menu` machinery and feel as EasyPanel's
  per-row menus, where those actions previously only had bare keys. (The Records screen
  keeps `Space` for its bulk menu; a per-record right-click menu there is a separate
  future item.)

## [0.83.3] — 2026-07-22

### Fixed

- **The `?` help no longer lists keys that do nothing in the Cloudflare workspace.** Its
  "Anywhere" and "Mouse" sections were the EasyPanel ones, so inside the CF workspace the
  help advertised `1-8 / Tab / ←→` tab-switching, the `:` command palette, the `s` server
  list, a "Click tab", and a right-click action menu — none of which act there. The CF
  help now shows only what works in that workspace (`W`, `?`, `r`, `Esc`, `q`; click-to-
  select and scroll), completing the workspace-accurate help.

## [0.83.2] — 2026-07-22

### Changed

- **The mouse now works in the Cloudflare TUI workspace, like it does everywhere else.**
  The scroll wheel scrolls the CF zones/records list under the cursor, hovering selects
  the row, and a left-click selects it — the CF tables were previously ignoring the mouse
  entirely (the shared mouse layer only ever addressed the hidden EasyPanel screen behind
  the workspace). EasyPanel's mouse behaviour is unchanged. (Right-click has no effect in
  the CF workspace yet — those screens have no per-row context menu; a future parity item.)

## [0.83.1] — 2026-07-22

### Changed

- **The Cloudflare TUI workspace now mirrors the EasyPanel TUI more closely.** Three
  parity fixes so switching between the two (with `W`) feels like one tool:
  - The header reads **"Cloudflare — <account>"**, exactly like EasyPanel's
    "EasyPanel — <server>", with the active account switched by the `a` picker the way
    `s` switches servers.
  - The header's second line is now a **product tab bar** (styled like EasyPanel's tab
    bar) — **DNS** today, built to grow into the other Cloudflare products (D1, R2, KV,
    Workers, …). The per-screen key hints moved from the header **into the status bar**,
    matching EasyPanel.
  - **`?` now opens the help overlay inside the Cloudflare workspace** (it used to do
    nothing there), and its "this screen" section documents the Cloudflare screen's keys
    instead of the stale EasyPanel one.

## [0.83.0] — 2026-07-22

### Added

- **Cloudflare — manage zones and DNS records, right beside your servers.** A whole
  capability that is deliberately OUTSIDE EasyPanel's scope: point the tool at one (or
  several) Cloudflare accounts and manage their zones and DNS records without leaving
  the terminal. It exists because migrating a service between hosts means repointing
  DNS, and doing that in a browser, one record at a time, is the slow part.
  - **Accounts are standalone and multiple.** Stored in their own `~/.config/easypanel/
    cloudflare.json` (0600, same corrupt-file guard as `servers.json`), independent of
    any EasyPanel server — an operator may hold several Cloudflare accounts. A scoped
    **API Token** (not the global key); the token is masked in every listing and never
    printed.
  - **CLI:** `easypanel cf account add/list/use/delete`; `cf zone list/add/delete`
    (deleting a zone asks you to type its name — it destroys every record in it);
    `cf record list` (filter with `--type/--name/--content`, pushed to Cloudflare's
    server-side filter so a zone with thousands of records returns only the matches),
    `cf record add`, `cf record delete`, and the headline **`cf record set`** — a bulk
    edit that changes one field on a *selection*: `cf record set example.com
    --where-content 203.0.113.10 --content 198.51.100.20` repoints every record off an
    old IP in a single command, with a per-record pass/fail report.
  - **TUI:** press **`W`** to switch into an isolated, Cloudflare-orange workspace
    (the EasyPanel tabs and the 1–8 keys are inert inside it, and vice-versa). Its home
    is the active account's **Zones**; `a` opens an account picker that mirrors the
    server switcher (select / add / delete); **Enter** on a zone drills into its
    **Records**, which support add, edit, delete, a `/` filter, and bulk change by
    marking rows with `v`/`V` then a `Space` menu. Empty, loading, and failed states are
    always distinguished — it never shows "no records" over a failed fetch.
  - Record edits use Cloudflare's **PATCH** (partial update) so changing one field never
    wipes the others; every endpoint shape was checked against the official Cloudflare
    API reference. v1 covers the common record types (A, AAAA, CNAME, TXT, NS, MX).

  **Verification note:** the request/response plumbing and error handling are proven
  against the live Cloudflare API (an invalid token surfaces Cloudflare's real error
  envelope), and all the request-building, filtering, and bulk-selection logic is
  unit-tested against the documented shapes. The full create/read/update happy-path
  should be confirmed against your own account the first time you use it — the tool
  cannot reach Cloudflare without your token, so a handful of scoped-token edge cases
  (e.g. exact zone-create requirements) are validated on first real use.

## [0.82.2] — 2026-07-22

### Fixed

- **The help/status hint now says the number keys go 1–8, not 1–7.** An eighth tab
  (Uptime) had been added with its own `8` shortcut, but the on-screen hint still read
  "1-7 / Tab / ←→", so anyone reading the help never learned they could jump straight
  to Uptime with `8`. The hint now covers all eight numbered tabs, and a test derives
  the range from the tab list so a future tab can't outrun it again.
- **Restoring a database from object storage no longer reloads every service.** The
  TUI restore returned `Refresh::Projects`, refetching the whole service list on
  completion — but a restore imports rows *into* a database and changes nothing in
  that table (names, status, metrics are untouched). It now uses `Refresh::None`, the
  same as the dump beside it: no wasted round-trip and no needless table churn on a
  large host.

## [0.82.1] — 2026-07-22

### Fixed

- **A failed dump or restore no longer leaves its temp file in the container.** The
  dump buffers to the container's `/tmp` and removed that file with `&& rm` — which
  runs only if every step *succeeded*. So a **failed upload** (the exact case that
  hangs least gracefully) left the gzip, roughly the dump's compressed size, sitting
  in `/tmp` — and repeated failures could fill it. Cleanup now runs whatever happens
  (`…; ec=$?; rm -f <files>; (exit $ec)`), and the command's real exit status is
  preserved so a genuine failure still surfaces. The database itself was, and stays,
  never touched — the dump only reads it. (Now safe to write this way because commands
  travel as WebSocket input since 0.82.0, not baked into the length-limited URL.)

## [0.82.0] — 2026-07-22

### Fixed

- **Dumping several databases at once no longer hangs.** Dumping one database
  worked, but selecting a few of them (or any dump whose command grew long) failed
  with "Dump did not report completion within 10 min" — while the databases were
  never locked and nothing was wrong with the data. The in-container command was
  passed inside the WebSocket connection URL, and a multi-database command (several
  schema names plus the ~380-character presigned upload URL) overran it: the command
  arrived truncated and the shell sat waiting on an unterminated line, so the dump
  never actually ran. The command is now sent to the container as terminal **input**,
  which has no length limit — the same channel the interactive shell already uses.
  Verified live: a four-database, ~100 MB dump that used to hang for ten minutes now
  finishes in ~35 seconds. (Found by an operator dumping a production database.)

### Added

- **Restore an object-storage dump from the TUI, and `easypanel db list`.** v0.81.0
  put the non-locking *dump* in the TUI but left *restore* to the CLI. Now a
  mysql/mariadb service's **Storage ▸** menu has **"Restore from an object-storage
  dump"**: it lists the dumps this tool has written for the service and restores the
  one you pick — recreating the databases even on a host that never had them. Since
  EasyPanel has no endpoint that lists these files, the tool signs an S3
  `ListObjectsV2` itself; the new `easypanel db list <project> <service>` prints the
  same list on the command line. The dump/restore/list orchestration is shared
  between the CLI and the TUI so the two can't drift.

## [0.81.0] — 2026-07-22

### Added

- **The non-locking dump to object storage is now in the TUI, not just the CLI.**
  v0.80.0 added `db dump`/`db restore` but only wired them into the CLI — so an
  operator working in the TUI still got EasyPanel's native backup, the one that
  **locks the running database**. That gap is closed: a mysql/mariadb service's
  **Storage ▸** menu now offers **"Dump now (non-locking) → object storage"** right
  above the native "Backup now". It reuses the same database picker (tick one, some,
  or *All*), then dumps every chosen database into ONE gzip file straight to your
  remote storage provider — non-locking, one request instead of one-per-database.
  The dump/restore orchestration is now shared between the CLI and the TUI so the
  two surfaces can't drift again. Verified live: dumped two seeded databases from
  the TUI into a single file on R2 and confirmed the table and its row were in it.

  (Restoring one of these dumps from the TUI is the next step — for now use
  `easypanel db restore`; the TUI's existing restore paths cover EasyPanel's own
  backups. The dump is mysql/mariadb only: it uploads with `curl` from inside the
  container, which those images ship and the postgres image does not.)

## [0.80.0] — 2026-07-21

### Added

- **`db dump` / `db restore` — a non-locking database backup to your object
  storage that actually restores onto a fresh server.** EasyPanel's own database
  backup has three problems an operator hits hard: it **locks the running
  database** (no `--single-transaction`, so apps using it error out during the
  backup), it writes **one file per database**, and its restore only works **into
  a database that already exists** — so moving a backup to a new host fails. This
  adds our own path that fixes all three:

  ```bash
  easypanel db dump <project> <service> --databases a,b,c   # or --all
  easypanel db restore <project> <service> --path <key>
  ```

  It runs `mysqldump --single-transaction` **inside the service container** (no
  lock), gzips it, and uploads it straight to your existing remote storage
  provider (e.g. Cloudflare R2) with a presigned URL — the data goes
  container→storage directly, so it never crosses this tool's WebSocket or a
  proxy's ~125 s timeout. Because the dump uses `--databases` (which embeds
  `CREATE DATABASE`), `db restore` recreates the schema **and its data on a host
  where it never existed** — the exact cross-server case EasyPanel can't do. One
  self-contained `.sql.gz` can hold several databases. mysql and mariadb today
  (postgres/mongo later). `--all` dumps every non-system schema the service holds.

  Verified end-to-end on throwaway services: dumped a seeded database to R2 and
  restored it into a *different* service that had never held it — the table and
  its row arrived intact. The AWS Signature v4 presigner is checked against AWS's
  published test vectors; database names are gated to a safe charset before they
  reach the container shell; and credentials (storage secret, DB root password)
  are never printed, including in error output.

## [0.79.4] — 2026-07-21

### Changed

- **The Domains screen no longer shows a bare `(1)` after single-server
  destinations.** A custom destination's servers each carry a load-balancing
  weight, and every destination — even one with a single server — was rendered as
  `https://webapp.harisenin.com (1)`. A weight only means anything relative to the
  other servers in the set; on a lone server it is always 100% of traffic, so the
  `(1)` was an unexplained token on the one screen you read to confirm where a
  domain actually points. Weights now appear only when there are two or more
  servers to weigh against (e.g. `a.test (1), b.test (2)`), where the split is
  real information.
- **Project header rows in the Services list dropped the row of `-` placeholders.**
  Each project header aggregates its services' CPU/memory/network, but it was also
  filling the Type/Status/Repl/Source/Auto columns with `-` — five dashes per
  project, a band of noise sitting between the project name and the aggregate
  numbers that are the row's whole point. A header has no single type or status,
  so those cells are now blank and the name + totals read as one clean summary
  band. (The metric columns still show `-` for "nothing measured", where the dash
  carries meaning.) Both surfaced by an on-screen UX critique and verified live.

## [0.79.3] — 2026-07-21

### Fixed

- **Restoring a backup into a service that never held that database now fails with
  a clear reason instead of a cryptic docker error.** EasyPanel restores INTO an
  existing database, so restoring `hscom_main` onto a fresh host — where it was
  never created — died with `[400] … docker exec … mysql … exit code 1`, which
  tells an operator nothing about what went wrong or how to fix it. This is the
  common cross-host case: move a backup to a new server and the target database
  does not exist there yet. Both restore paths (the TUI and `backup db-restore`)
  now ask the service what databases it actually holds (`getServiceDatabases`)
  and, if the target is not among them, refuse up front with
  `"<service> has no database '<db>'. EasyPanel restores into an existing
  database — create '<db>' in <service> first, then restore."` — before any
  overwrite is attempted. An empty or unreadable listing (e.g. a stopped engine,
  which never reports even its own system schemas) is treated as "can't tell" and
  lets the restore proceed, so the check never blocks a restore that might work.
  Reported by an operator restoring across hosts; the failing service and the
  fixed behaviour were both verified live.

## [0.79.2] — 2026-07-21

### Fixed

- **The "restore from another server" list now shows which database each backup
  holds.** A database service can hold several databases (schemas), and EasyPanel
  backs each up to its own file — so the list was full of rows that all read
  `harisenin-com-db/mysql` with no way to tell `hscom_core` from `hscom_learning`
  apart, or to know what a given file would restore. The database name was already
  carried for the restore itself; it is now a column (When · From · **Database** ·
  File), so you can pick the right backup by what is actually inside it. Reported by
  an operator restoring across hosts; verified live. (The single-service restore
  already showed the database; only the cross-host list dropped it.)

### Fixed

- **The create-service wizard lets you make an app with no source yet.** The
  Source step gate validated through `source_body` (which requires a repo/image),
  while the submit path and the code's own docs say an app may be created as a
  shell and its source wired up later — so a valid, panel-supported workflow was
  dead-ended with a "Repo must be selected" message that read as a requirement. The
  gate now uses the same lenient `create_source` the submit uses: an untouched
  source advances, a HALF-formed one (a repo with no branch) is still refused on
  its own step. Found by an adversarial UI review; verified on screen. One new test
  drives the step transition; the older wizard test was realigned to demonstrate
  its point (no deferred validation) with a half-formed source rather than an empty
  one.

### Fixed

- **The project → service hierarchy is visible again in the tables.** In the
  Monitor (and, more subtly, the Services) table a service is meant to sit
  indented under its project header, but `first_line` — the shared cell truncator —
  did a full `.trim()`, eating the two-space indent, so on a busy host every row
  read at the same level and a project header looked like just another service.
  `first_line` now trims only the trailing side (leading whitespace is meaningful
  indentation), and project-header rows are drawn in **bold cyan** on both tables so
  the grouping reads at a glance. The header cue comes from the row's own type, not
  from testing the indent, so a marked service (`✓ name`) is never mistaken for a
  header. Reported by an operator looking at a 118-service host; verified on screen.

### Fixed

- **Services and the Dashboard no longer lie when their fetch fails** — the same
  bug v0.78.1 fixed on Domains, found on its two worst siblings by an adversarial
  audit of every data screen. On a failed load (e.g. a gateway 502): the **Services**
  screen drew a bare empty table that read as "this host has nothing" on a host
  with hundreds of services; the **Dashboard** was worse still — it drew CPU/memory/
  disk gauges at a fabricated **0.0%**, numbers that look real and are at once
  alarming ("disk empty?!") and falsely reassuring ("CPU idle"). Both now say
  "⚠ Couldn't load … — <error>. Press r to retry" and keep any data already loaded
  (a refresh failure preserves last-good values). Verified live against an
  unreachable host. The audit confirmed the other data screens are fine: the
  service-detail collections open only on success, and the Hosts screen already
  renders per-host failure — so they were left untouched.

### Fixed

- **The Domains screen no longer reads "No domains yet" when the fetch failed.** A
  gateway 502 (seen live on a host with hundreds of domains) left the list empty,
  and the empty-state drew "No domains yet — press n to add one" — as if the host
  had none. A failed load is now tracked apart from a genuinely empty one, so the
  screen says "⚠ Couldn't load domains — <error>. Press r to retry" and keeps any
  domains it had already loaded. Found by driving the biggest live host.
- **"Esc back" on the Credentials screen now actually goes back.** Credentials is
  opened from the (usually filtered) Services list, and the global Esc handlers —
  "clear the filter", "clear the marks" — fired first, so Esc silently cleared the
  filter or *destroyed the marked set* and stayed on the screen; the advertised
  "Esc back" did nothing until a second press. Full-screen sub-views (Viewer,
  Credentials) now own their Esc: it returns to the list with the filter and marks
  intact. The reveal (`v`) status hint also stays in step with the masked/revealed
  state instead of going stale. (Found by an adversarial UI review.)

### Added

- **Bulk-set resource limits across many services at once.** Mark services (`v`,
  or `V` for every row the filter shows), open the action menu, and pick **Set
  resource limits on N marked services** — one form for CPU/memory limit and
  reservation, applied to all of them under each service's own type endpoint.
  Setting the same cap on thirty services used to be thirty trips through the
  single `L` form; now it is one. A marked service whose type keeps its limits in
  a compose file (not the API) is reported by name in the result, never silently
  skipped, so the summary accounts for every service you marked. Like the single
  form, `updateResources` only stores the values — a deploy applies them. Verified
  live: the same limit round-tripped onto three services via one submit.

### Fixed

- **Credentials: a root-only database no longer shows `User = -`.** The fallback
  to `root`/`postgres` was decided with `field(...).is_empty()`, but `field`
  returns the string `"-"` for a MISSING key, not `""` — so a database whose
  `inspectService` omits the `user` key (a root-only setup) took the app-user
  branch and rendered `User = -`, `Password = -`, and a `mysql://-:-@…` URL. The
  `"-"` sentinel is now normalised to empty (the same guard the rest of the code
  uses), so an absent field falls back correctly. Found by an adversarial review
  of the v0.77.0 change.
- **Credentials: the masked password no longer hints at its length.** The bullet
  run was sized from the value's own length for 8–24 char secrets, so the number
  of `•` usually equalled the real password length. It is now a fixed width.

## [0.77.0] — 2026-07-21

### Added

- **See and copy a database's credentials, in the TUI.** On any database service
  (mysql, mariadb, postgres, mongo, redis) the Shell menu (`t`) now has a
  **Credentials** entry that opens a read-only view of the connection identity the
  panel shows: user, password, internal host, internal port, and a ready-to-paste
  connection URL. The tool already knew these — the DB shell (`y`) logs in with
  them — but there was nowhere to just read one off or grab it for a client.

  Secrets stay **masked by default** (a password on screen is a deliberate act in
  a tool that redacts everywhere else): `v` reveals or hides them, and `c` / `y` /
  `Enter` copies the selected field's real value — even while masked — to the
  system clipboard. Copy uses OSC 52, so it reaches the clipboard even over SSH and
  through tmux (`set-clipboard on`), which is where this tool tends to run; no new
  dependency, no `pbcopy`/`xclip` shelling. The connection URL percent-encodes the
  user and password so a password containing `@`, `:` or `/` still parses. Verified
  live against redis and mysql on a real host.

### Fixed

- **A wildcard domain now shows its `*.` prefix.** EasyPanel stores `*.edu.example` as
  `{ host: "edu.example", wildcard: true }` — the star is a separate flag, not part of the
  host — and the tool was ignoring that flag. So a wildcard domain and its apex rendered
  **identically** (`https://edu.example/` for both): two genuinely different routes that
  looked like one duplicate row, on the screen you go to precisely to tell domains apart.
  A wildcard host now reads `https://*.edu.example/`, matching the panel's own UI. The fix
  is in `domain_source`, so it flows to the TUI Domains screen, the CLI `domain list`, the
  cross-host domain diff, and `project export` at once. Found by driving the live host.

## [0.75.0] — 2026-07-21

### Added

- **Export a project's config to a git-committable file.** `easypanel project export
  <name>` writes every service's source, build, deploy block, resources, env keys,
  domains, mounts and ports to `<name>.easypanel.json` (or stdout with `--file -`).
  EasyPanel has no export and no import at all, so until now there was nowhere to get your
  config for a review, a record, or a diff across time.

  It is built to be **safe in git and stable to diff**: env is reduced to its KEYS — never
  a value, since an env is the densest pile of secrets a service has — the deploy token is
  dropped, and any secret-named field like a private registry `password` is masked, the
  same rule the on-screen views enforce. Volatile per-deploy noise (the last commit hash,
  the deployment URL, the primary-domain id) is left out, so a diff shows configuration
  changes rather than deploy churn. Config only: no data, no secret values, ever.

  This is export only; applying a file back to a host is a separate, deliberate step not
  built yet. Verified live that a registry password and env values never reach the file.

## [0.74.0] — 2026-07-21

### Added

- **Compare a WHOLE project across two hosts.** v0.72.0 compared one service on two hosts;
  this compares every service in a project at once — "is staging actually in sync with
  production?". On a project (or any service in it), pick "Compare WHOLE project with
  another host". It surfaces, in one screen:
  - services that exist on **only one** host — the drift a service-by-service compare can
    never show you, and usually the one that matters (staging is missing the worker prod
    has);
  - services that exist on both but **differ**, with a count of differing fields, worst
    first, so you know which one to open;
  - the services that are identical.

  Just two API calls — `inspectProject` already carries every service's full config, so no
  per-service round-trip. The per-field engine is the one v0.71.0/v0.72.0 proved, so env
  is still compared by key with values never shown. A failure names which host it came
  from. Nothing in the web panel can look at two hosts at once.

## [0.73.0] — 2026-07-21

### Fixed

- **A viewer now says when a line runs off the right edge.** A diff or a bulk-result line
  wider than the pane was cut at the border with nothing to say it could scroll — the
  "← col N" indicator only appeared AFTER you had scrolled, which you had no reason to try.
  A `→ more · ←→ scroll` hint now shows at the bottom-right exactly while a line is cut and
  you have not yet scrolled, and disappears both when it fits and once you start scrolling.
  Found by driving the new two-service diff at 78 columns.

## [0.72.0] — 2026-07-21

### Added

- **Compare a service against the same one on another host.** v0.71.0 compared two
  services on one host; the sharper question is "staging vs production" — the SAME service
  on two different machines. On any service, "Compare with another host" (in the `Space`
  menu) picks another configured server and diffs the same project/service fetched from
  there. Nothing in the web panel can look at two hosts at once.

  The engine is the one v0.71.0 already proved — env by key, values never shown,
  order-independent — so only the second fetch is new, using the target host's own token
  (resolved the same way migration resolves it). A failure names WHICH host it came from,
  because "not found" usually means the service does not exist on the other side and the
  operator needs to know which side that is.

### Fixed

- **An empty value and an absent one no longer read as a difference.** A `deploy.command`
  of `""` on one service and unset on another both mean "no command"; showing them as a
  difference (`  →  —`) is the noise that trains a reader to stop trusting the diff. Both
  now fold to "not set". Found on a real cross-host compare.

## [0.71.0] — 2026-07-21

### Added

- **Compare two services.** "Staging works and production doesn't — what is actually
  different?" is a question an operator asks constantly and today answers by opening two
  screens and reading them line by line. Mark two services (`v` on each), open the menu
  (`Space`) and pick **Compare the 2 marked services**. It shows every field that differs
  — type, source, build, deploy block, resource limits — and the environment compared
  **by key**: which variables differ, which exist on only one side, which agree.

  The env values are never shown, only whether they match. An environment is the densest
  collection of secrets a service has, and printing `DATABASE_URL: postgres://user:pw@… →`
  to answer "are these the same?" would leak exactly what the recent credential-masking
  work exists to contain. Comparison is also order-independent, so a reordered env line
  does not read as a change.

  Everything needed was already fetched (`inspectService`, which powers clone and
  migrate); this only decides what differs. Two identical services get a plain sentence
  saying so — itself the useful answer, pointing you outside the config to data or the
  host. Nothing the panel offers.

## [0.70.0] — 2026-07-21

### Added

- **`f` on the Actions tab shows only what did not finish cleanly.** Colouring the
  statuses (v0.68.0) made failures visible, but finding them still meant typing into the
  text filter — which also matches commit messages, so searching `error` returned
  successful deploys whose message happened to contain the word. `f` keeps only `killed`,
  `error` and still-running actions, hiding every clean `done`. The title says `failures
  only` and switches its count to the shown total, so a filtered list never reads as
  missing data. `f` again brings everything back.

## [0.69.0] — 2026-07-21

### Added

- **Domains pointing at a service that no longer exists are marked.** Rename or destroy a
  service and its domains stay behind, still listed, still looking healthy — the panel has
  no idea the destination is gone. The Domains tab now marks those rows `✗` in red and
  says how many there are in the title.

  Measured before building it, on a live host: **one of 713 domains** was dead. That is
  precisely the case for having it — a single broken route is invisible among seven
  hundred working ones, and it is the kind of thing nobody notices until a deploy quietly
  stops arriving somewhere.

  The mark sits at the FRONT of the destination so it survives truncation and reads
  without colour, in a pipe or a black-and-white screenshot. Two things are deliberately
  never judged: a custom destination, which points at a URL this tool knows nothing
  about, and **anything at all before the service list has loaded** — judging against an
  empty list would condemn every domain on the panel at once, which on that host is 713
  confident wrong answers.

## [0.68.0] — 2026-07-21

### Fixed

- **A failed deploy looked exactly like a successful one.** The Actions tab — the screen
  whose whole purpose is "what happened" — drew every row in the same grey. Colour
  carries state everywhere else in this tool: an unreachable host is red, a crashed
  service pulses, a swarm node that left the cluster is red, an uptime check is green or
  orange or red. History was the one screen that said nothing.

  It matters more than it sounds. On the owner's own panel, **19 of the last 200 actions
  were `killed` or `error`** — a tenth of the screen was findings nobody could see
  without reading each row.

  The status now uses the palette the rest of the app already uses: green for `done`,
  yellow for `killed` (deliberately halted — the same yellow `stopped` gets on Services),
  red and bold for `error` (it failed on its own), cyan for `running`. A status EasyPanel
  adds later is left unpainted rather than guessed at.

- **"1 minutes".** Durations always pluralised, so a deploy that took a minute read
  `1 minutes` and an action from an hour ago read `1 hours ago` — on two columns of a
  screen full of them, in the TUI and the CLI alike.

## [0.67.0] — 2026-07-21

### Security

Two findings from a dedicated audit of one question: *where else can a credential
reach somewhere it should not?* Both are the same mistake v0.66.0 fixed for fields,
in the two places a field-name check cannot see — a secret inside a **URL**, and a
secret inside a **text blob**.

- **A failed terminal connection printed the API token on screen.** The container
  shell's WebSocket URL carries `?token={your API token}`, and for a database shell
  the base64 of a command containing the root password. When the connection failed,
  the library's error renders as *"Unable to connect to {the whole URI}"* — and that
  went straight to the status line. Any panel outage, wrong port or firewalled host
  was enough, and the token stayed in the terminal's scrollback afterwards, ready to
  be pasted into a bug report. The failure now says it could not reach the panel; the
  error variants that do NOT carry the URI keep their own message, since those are the
  useful ones.

- **`$EDITOR` temp files were world-readable.** Editing a service's env, a project's
  shared env, a database config file or an uptime check's headers writes the contents
  to a temp file first. On Linux that is the shared `/tmp`, at default permissions,
  under a fully predictable name — so any account on the machine could read a
  service's entire environment: connection strings, third-party keys, signing secrets.
  Denser than any single field. The file is now created `0600` before a byte is
  written, and a stale file is replaced rather than written through.

A third path was found and deliberately NOT changed: the database shell's password
rides in the WebSocket query string, so a proxy in front of the panel will have it in
its access logs. Fixing that needs EasyPanel to accept the command as a post-connect
frame; it is recorded in the brief rather than papered over here.

### Fixed

- **A table cut its widest column without saying so — for the fifth time.** Found on
  the Monitor tab: two storage paths differing only in their last character both
  rendered as `…/mysql-r`, so the column that identifies the row said nothing while
  looking complete. The arithmetic for "how wide does the flexible column actually
  get" had been written by hand in five places and forgotten in this one, so it now
  lives inside `render_table` itself: every table drawn through it is cut with an
  ellipsis, including tables nobody has written yet.

## [0.66.0] — 2026-07-20

### Security

- **The Source & build view printed registry credentials in the clear.** A service
  pulling from a private registry stores a `password`, and that view showed it in full —
  found on a live host, where it was a real GitHub token readable by anyone looking at
  the screen or a screenshot.

  The view already skipped the service's `token` and `env` keys, so credentials had been
  thought about; the list simply missed one. And the edit form for that very field has
  always masked it, so one fact was rendered two ways in the same application.

  Fields are now judged by NAME — anything containing `password`, `token`, `secret`,
  `credential`, `apikey` or `privatekey` is masked as `••••••••`. Masked rather than
  dropped, because knowing a password IS set is useful. Matching on the name also means a
  secret field added by a future EasyPanel arrives hidden instead of exposed, which a
  fixed list of exclusions can never do.

  **If you have opened this view on a service with a private registry, treat that
  credential as exposed and rotate it.**

## [0.65.2] — 2026-07-20

### Fixed

- **The command palette found nothing until you had opened the Services tab.** `:` is
  the "jump to any service from anywhere" key, but the service list was only fetched when
  that tab was visited — so on a fresh launch, searching for a service that plainly exists
  answered `0 results`. That is worse than an empty box: it is a confident wrong answer,
  and the natural conclusion is that the service is gone.

  The list is now fetched at start-up, for exactly the reason the storage providers
  already were: the palette needs it the moment the key is pressed. First arrival at the
  Services tab is quicker as a side effect.

## [0.65.1] — 2026-07-20

### Fixed

- **Enrolling a domain for uptime checks now asks what to check it with (reported by the
  owner).** `w` used to enrol instantly with a silent GET, leaving the method, body and
  expected status to be set later on another screen — two doors into one room, and a
  deliberate act performed without the user deciding anything. `w` now opens the check
  form, and nothing is stored until it is saved. On a domain already watched it reopens
  that same form to edit it.

- **Two dialogs were sized by a number that meant something else.** `render_form` and
  `render_confirm` each measure the width they need in COLUMNS and then handed it to a
  helper whose first parameter is a PERCENTAGE — two functions one character apart in
  name. At 80 columns a form that measured itself at 68 was drawn 54 wide, cutting the
  very text the measurement exists to protect: the "Watch …" form lost the second half of
  the URL it was about. Worse for the confirmation, which wraps its label using a width
  it never received, so the label ran to more lines than the box allowed and the
  `[y] Yes  [n] Cancel` line could fall out of the bottom — on the dialog that guards
  irreversible actions.

- **The tab bar lost a tab at 80 columns.** Adding the eighth tab in v0.65.0 pushed the
  full labels past the frame and the strip was clipped, so `Uptime` rendered as `Up`. The
  two longest labels now shorten to `Dash` and `Maint` when the full set will not fit —
  a shortened word beats a missing tab, on the one strip whose job is saying where you
  are and where you can go.

## [0.65.0] — 2026-07-20

### Added

- **Uptime checks for the domains you choose (new `Uptime` tab, `8`).** Requested by the
  owner. A domain in the panel is a routing rule, not a promise: it can point at a service
  that was renamed, a port nothing listens on, or a container that stopped, and the panel
  will keep showing the rule as if it were fine. This asks the domains themselves.

  **Only what you enrol is watched.** `w` on a domain adds it, `w` again removes it. On a
  host with 700 domains most are aliases and parked names, and a list that watches
  everything is a list nobody reads — so the watchlist is short, curated, and yours. It
  is stored per server in `~/.config/easypanel/checks.json` at mode `0600`, because a
  check may carry an `Authorization` header.

  `e` edits the request: any method with a body and headers, a timeout, and the status
  you *expect*. That last one matters — an API answering `401` to an unauthenticated
  probe is behaving correctly, and treating it as an outage is the false alarm that
  teaches people to ignore alarms. Redirects are counted as working and deliberately not
  followed: the question is whether THIS domain and path answer, and following the hop
  would report on a different URL and time it too.

  Latency is split into the wait for the response head — the server thinking — and the
  total including the body. A high first number with a fast finish is a slow application;
  a fast start with a slow finish is a big payload or a slow link. And because the whole
  watchlist is checked at once, each domain is compared against the median of its peers,
  so the answer is "3.2× slower" rather than a millisecond count nobody can calibrate.
  The median, not the mean: a single timeout would drag a mean past everything genuinely
  slow. None of this needs any stored history.

  Broken domains sort first, then the slowest. Found two real problems on the owner's own
  host on its first run.

## [0.64.1] — 2026-07-20

### Fixed

- **Filtering a long list could show one row under a title claiming hundreds.**
  Found by driving a real host with **713 domains**: scroll to the bottom, then filter
  to `viding.co`, and the heading read `452/713` above a single row and nineteen blank
  lines. The screen contradicted its own heading, and the obvious reading is "the filter
  is broken" or "the data is gone".

  Changing the filter only clamped the selected index into the shorter list. The scroll
  offset is kept separately and ratatui only moves it when the selection sits above it,
  so a view scrolled near row 700 stayed there while the list underneath became 452 rows
  long. Keeping that index was not even meaningful — row 451 of a filtered list is a
  different row from row 451 of the unfiltered one.

  The view now starts at the first match on every keystroke, which is what you want when
  narrowing a list. The arrows still work while you type.

## [0.64.0] — 2026-07-20

Five defects found by critiquing the render layer and then driving the binary to
confirm each one on screen. They share a theme: **the same fact told two ways, or
cut without saying it was cut.**

### Fixed

- **The Services table cut a repo name with no ellipsis.** `harisenincom/edukasistudio`
  rendered as `harisenincom/edu` — a name that looks complete and is a different repo —
  and seven services of one project all read `harisenincom/har`, telling the reader
  nothing while appearing to. Name and Source are both flexible columns, so ratatui split
  the leftover between them and neither cell knew how wide it ended up. The Source width
  is now worked out up front and the cell cut to it, with the ellipsis the Actions and
  Domains tables already use. Wide terminals still show the whole thing.

- **A long host name squeezed the column that says WHY a host is unreachable.** The
  Hosts table's drop thresholds were written when the Server column was a fixed 14
  characters. Making it fit real names (v0.63.0) left every threshold one column
  optimistic, so at some widths the last column was kept when it no longer fit and
  Status — the only flexible column — silently absorbed the deficit. The thresholds now
  move with the Server column. The failure reason is also cut at the width Status
  actually gets: it was being trimmed at 40 characters, wider than that column is ever
  given, so the trim never fired and the cause was clipped at the column edge instead —
  reading as the whole reason when it was the first half.

- **A Swarm node that left the cluster looked like a healthy one.** Colour carries state
  everywhere else — an unreachable host is red, a crashed service pulses red — but the
  Nodes table on the Dashboard, the first screen on launch, painted `down` as ordinary
  text. A node leaving the cluster is why services vanish.

- **CPU read `11.9%` on three screens and `11.9 %` on two others**, and the Services
  table put `0.4 %` under a header already reading `CPU %`. One number now reads one way,
  in the TUI and the CLI alike.

- **The Source & build view printed raw JSON booleans.** `autoDeploy: true` — the same
  field the Services table shows as `✓`/`✗` and the Backups view shows as `on`/`off`.
  Three renderings of one boolean in one app; this was the raw one. It now says yes/no.

## [0.63.0] — 2026-07-20

### Fixed

- **Switching host now outlives the session (reported by the owner).** Switching
  from one server to another and then quitting meant the next launch silently
  came back up on the OLD host. The switch was only ever held in memory; the
  config's default was never moved. That is precisely the wrong-machine mistake
  the per-server colours were added to prevent — the colour said "you are on
  angelia" right up until you restarted and it quietly said "aurel" instead.
  Switching is a deliberate "I am working on this machine now", so it is
  remembered. If the config cannot be written the switch still happens, and the
  status line says the switch worked but could not be remembered rather than
  pretending either half succeeded.

- **The Hosts table cut the one thing that identifies a host.** The Server column
  was a fixed 14 characters, so `angelia-machine` — fifteen — rendered as
  `angelia-machin`. It is now sized to the longest name actually configured, and
  clamped so a single very long name cannot push the metrics off screen.

### Added

- **A server can be renamed (reported by the owner).** The edit form offered only
  URL and Token, so a typo in a server's name was permanent: the only way to fix
  one was to delete the server — taking its token, which cannot be read back from
  anywhere, with it. The name is now the first field. A rename moves the entry in
  place, keeping its token, its default flag and its position in the list, and
  renaming onto a name that already exists is refused rather than merging two
  hosts into one entry pointing at the wrong machine.

## [0.62.0] — 2026-07-20

### Added

- **Edit a project's shared environment (`e` → "Project env").** EasyPanel lets a
  project hold variables that every service in it receives; nothing in this tool
  could read or change them. It sits behind the same door as a service's own env
  rather than getting a key of its own — it is the same idea one level up.

  Three things were unknown before this and are now verified live, which is why
  the brief had it blocked:

  - `inspectProject` **does** return the env, under `project.env`. It only looks
    absent because the key does not exist at all until the env is first set — so
    prefilling the editor is safe and does not edit blind.
  - The variables really do reach the containers: a project variable showed up
    inside a running container next to the service's own, through the tool's own
    container shell.
  - **A change does not take effect until the services are deployed again.** The
    running container still reported the previous value after the project env
    had been saved, and the new one only after a deploy.

  That last point is why saving is not the end of the flow. Instead of reporting
  "saved" and leaving containers quietly running the old values, the tool says
  how many services are now stale and offers to deploy them — counting only the
  ones that CAN be deployed, since a database or a wordpress has no build step
  and deploying it is a route that does not exist.

## [0.61.0] — 2026-07-20

### Added

- **Bulk domain edit (`E` on the Domains tab).** Moving a fleet to a new
  hostname, or repointing every domain that feeds a service being replaced, was
  the edit form opened once per domain — twenty chances to fat-finger one and
  not notice which. `E` rewrites one part across many domains at once: the host,
  the destination service (as `project/service`, so a domain can move project as
  well as service), or the URLs of a custom destination.

  Which domains it touches is the filter that is already on screen — `/` narrows
  first, and the form's title says how many are in range — rather than a second
  way of selecting things that exists only here.

  It is a plain find-and-replace, deliberately not a regex: these values are
  hostnames and service names, where a `.` typed as itself would quietly match
  any character and rewrite a domain nobody meant to touch.

  Nothing is sent until the whole before → after list has been shown and
  accepted, and a rewrite that would produce a broken domain — an empty host, a
  destination that is no longer a `project/service` pair — is refused for the
  whole batch rather than half-applied across a fleet. Everything the form does
  not model (middlewares, the certificate resolver, the weights and any further
  servers of a custom destination) is carried through untouched, and a custom
  destination's servers all move together so traffic cannot end up split across
  two hostnames. Verified live: the rewrite updated the domains in place, left
  the filtered-out domain alone, and left port, protocol and HTTPS as they were.

## [0.60.1] — 2026-07-20

### Fixed

- **A cut action description now says it was cut.** Found by driving the TUI at
  80 columns: a deploy read `Deploy service: feat: u` and stopped, which reads as
  the whole commit message rather than the first quarter of one. The row was
  trimmed at 200 characters — a limit a ~23-column cell never reaches — so the
  rest was clipped silently at the column edge.

  Every column on that screen is now cut at the width it will actually get, with
  an ellipsis, which is the rule the Domains screen already followed. On a wide
  terminal nothing is cut at all: the same message shows in full.

## [0.60.0] — 2026-07-20

### Added

- **Replicas can be changed, not just read.** The Source & build screen showed
  `replicas: 1` with no way to alter it. Build & source ▸ *Replicas & deploy* now
  opens the whole `deploy` block — replicas, start command and zero-downtime — in
  one form, prefilled from the service.

  The payload is nested under `deploy`; a flat `{replicas: 3}` is answered `200`
  and changes nothing, which would have looked exactly like success. That shape
  was found by trying it against a live panel, not assumed.

  Two guards, because this is a number that gets typed: anything that is not a
  whole number is refused by name (`Replicas must be a whole number, not 'dua'`),
  and `0` — which would quietly stop the service — points at Lifecycle ▸ Stop
  instead. `updateDeploy` exists only for `app` services; every other type
  answers the bare 404 of a missing route, so it is offered nowhere else.

  Verified live: 1 → 3 through the form, confirmed on the server, and both
  refusals seen on screen.

## [0.59.2] — 2026-07-20

### Fixed

- **The server's colour now covers every frame, not just the tab strip.** v0.59.0
  tinted the one small box at the top and left the large one filling the screen
  grey — a signal you had to look for rather than one you could not miss. The
  tables, the dashboard gauges and sparkline, the monitor tiles, the log viewer
  and the embedded terminal all carry it now. The terminal matters most of those:
  a shell on the wrong host is the worst thing on this list.

  Popups deliberately keep their own colours — a confirmation is yellow and a
  form is cyan because those mean something else — and the confirmation names the
  server in words anyway.

## [0.59.1] — 2026-07-20

### Fixed

- **The Domains screen stops cutting the destination on a wide terminal.**
  Reported from real use: at 186 columns a destination read
  `http://harisenin-com_miniapp-gopa…` while a 25-character cuid sat beside it at
  full width. Destination was pinned to 34 columns and Source absorbed every
  spare one, so the wider the terminal, the more room went to the column that
  needed it least.

  Source and Destination now share the spare width; only the ID stays fixed,
  because an opaque cuid is the one thing on that screen nobody reads or types.
  All 22 domains on a live panel now render in full, custom destinations
  included.

## [0.59.0] — 2026-07-20

### Added

- **You can see which server you are on, without looking for it.** With several
  hosts configured, the answer to "am I about to change the right machine?" lived
  in one small line in the corner — and at the moment it mattered most, the
  confirmation dialog, it was not there at all. A dialog asking to destroy a
  service named the project and the service, but never the host.

  Two changes, aimed at the two ways people actually notice things:

  - **Every confirmation names the server**, on its own line above the target and
    in that server's colour. It is the last thing read before pressing `y`.
  - **Each server has its own colour**, on the frame and its name. It is a pure
    function of the name, so a host looks the same every time you open it and
    different from its neighbours — you register the change before you read
    anything. The palette avoids the colours that already mean something here
    (red for down, green for active, pink for errors).

  Verified on two live hosts: the frame is blue for one and magenta for the
  other, and stopping a service on the second asks "on angelia-machine" before
  the target.

## [0.58.0] — 2026-07-20

### Added

- **A mount can be edited, not just added and deleted.** Changing where a volume
  landed meant deleting it and building it again — an alarming thing to do to a
  volume. `e` in the Mounts viewer opens the mount under the cursor, prefilled,
  and saves through `updateMount`. The values are fetched at the moment you press
  `e` rather than remembered from when the list was drawn, so editing one someone
  else has changed since fails honestly instead of overwriting them; a mount that
  has since disappeared says so. The list reloads on save, because a change you
  cannot see is indistinguishable from one that did not happen.

  `e` keeps one meaning per screen: it lives in the same handler as the viewer's
  other letters instead of a second arm that shadowed Env's and Source's `e`.

### Fixed

- **A rejected form now says which field, and why.** Every validation failure
  reported `[400] Input validation failed` — the generic wrapper — while the
  sentence the user needed sat untouched in `data.zodErrors`. Typing a volume
  name with a capital letter in it now answers:

  > name: Invalid name. Use lowercase letters (a-z), digits (0-9), dash (-),
  > underscore (_).

  This applies to every form in the tool, not only mounts. The field name is
  taken from the leaf of the error, so the request's own nesting ("values") never
  reaches the user.

## [0.57.2] — 2026-07-20

### Fixed

- **A domain is no longer offered on a service that cannot have one.** Pointing a
  domain at a database is refused by the panel — `createDomain` answers
  `Wrong service type.`, verified live against real mysql and redis services
  while an app was accepted — yet "Domain" appeared in the Networking menu of
  every service. It now follows the same web-only rule that already governed
  redirects and basic auth.

  The domain form's destination dropdown was the second way in: it listed every
  service in the project, databases included. It now offers only the ones a
  domain can actually reach — in a project holding four databases and one app,
  the dropdown is that one app.

  Three copies of "which types serve HTTP" existed across the menu, the form and
  the new check; they are one list now.

### Note

This is the fourth entry in the same class — after lifecycle actions (v0.52.0),
backups (v0.55.1) and mounts/ports (v0.57.1) — but the first found by auditing
every menu entry against the API rather than by tripping over it. Two suspects
came out clean: `Env` works on redis (the field simply does not exist until you
set one), and the rest of the menu is type-appropriate. The verified per-type
matrix in `.github/AGENT_BRIEF.md` now covers domains too.

## [0.57.1] — 2026-07-20

### Fixed

- **Mounts and Ports are no longer offered where they cannot exist.** Opening
  Mounts on a MySQL answered `[400] Invalid service type` — the entry was shown
  on every service in the panel. Probed live against real services of each type:
  `mounts` and `ports` accept only `app` and `box`; every database, `compose` and
  `wordpress` refuse them. EasyPanel manages a database's storage itself, and a
  compose stack declares its own in its file. The `p` and `M` keys are guarded
  too, not just the menu — the leaf keys stay live where the menu hides an entry,
  which is how the Lifecycle menu stayed broken for databases until v0.52.0.

- **A group with nothing in it for this service says so.** Withdrawing both
  mounts and backups correctly left Storage empty on a redis, and pressing `m`
  then did nothing at all — the same dead end the gating set out to remove, only
  quieter. It now answers "Nothing here for a redis service".

- **A form is as wide as the note explaining it.** The box was a fixed 64 columns,
  so its own guidance was cut mid-word: "only ones on shared remote stora". A
  sentence that stops without warning is worse than none, because the reader
  cannot tell what was withheld.

### Note

A test had asserted that redis *has* ports — an assumption never checked against
a server, and wrong. It is corrected here. A test that encodes a wrong assumption
is worse than no test: it turns a bug into evidence of correctness.

## [0.57.0] — 2026-07-20

### Changed

- **A cloned or migrated database now keeps its config file.** Since v0.51.1 the
  config was held back entirely when it contained `read_only` / `super_read_only`
  — safe, but it threw away configuration the user had asked to copy, which the
  owner rightly objected to.

  The whole file is copied now. Only the two directives that stop a brand-new
  database from initialising are left as comments, with their original value
  intact after a `# easypanel-cli:` marker — so re-enabling them is deleting a
  prefix, not retyping a setting. Everything else (server-id, relay_log, tuning)
  arrives active, as it should.

  Verified live by cloning a real MySQL replica: 56 lines in, 56 lines out, two
  commented; the clone then initialised cleanly — `Creating database`,
  `Creating user`, `ready for connections`, and no `ERROR 1290` — and answered
  `SELECT @@server_id` with the replica's own id, using the password the panel
  recorded. Before this change that same clone came up with no root password, no
  user and no schema, and could not be repaired.

## [0.56.0] — 2026-07-20

### Added

- **Tick several databases and back up exactly those.** The picker offered "all
  or exactly one", so backing up three of five meant running the whole flow three
  times. `v` now ticks the database under the cursor — the same key that marks a
  service in the table, because it is the same idea — and Enter backs up
  everything ticked. Nothing ticked still means "the row you are on", and
  "All N databases" is still the first row.

  Ticks win over the cursor when both could apply: they were made deliberately,
  while the row the cursor happens to rest on is not a choice anyone made. The
  confirmation names every database it is about to touch, and each gets its own
  request, so one failing cannot quietly take the others with it.

  Verified live on a service holding three: two ticked, and exactly those two
  were backed up — with no schedule left behind.

## [0.55.1] — 2026-07-20

### Fixed

- **Redis is no longer offered a backup it cannot have.** Opening the database
  picker on a redis service listed a database called `-` — the placeholder for a
  value that isn't there, since redis carries no `databaseName` at all. Choosing
  it could only fail. Verified against a live panel: `createDatabaseBackup` on a
  real redis service answers `Service is not supported`, so Backup, Restore and
  Backup schedules are simply not offered for it. mysql, mariadb, postgres and
  mongo are all accepted and unchanged.

- A service whose database cannot be determined now says so, instead of offering
  "All 0 databases".

Found by looking at the screen after an unrelated refactor — the picker was
working perfectly, on a service that can never be backed up.

## [0.55.0] — 2026-07-20

### Added

- **Choose WHICH database to back up — including all of them at once.** Reported
  from real use: a MySQL service holds many databases, but "Backup now" only ever
  backed up the one EasyPanel recorded when the service was created, with no way
  to pick another.

  It now asks the engine what it actually holds and offers the real list, with
  "All N databases" first. Verified live on a service holding three: all three
  were backed up in one confirmation, each producing its own file of its own
  size, and no leftover schedules.

  Two things made this possible, both checked rather than assumed. The backup
  endpoint accepts ANY database name in the service, not just the configured one
  — backing up a schema the panel had never heard of produced a real dump of
  exactly that schema. And nothing in the API lists the databases inside a
  service, so the engine is asked directly through the container shell
  (`SHOW DATABASES` for MySQL/MariaDB, `pg_database` for PostgreSQL), skipping
  the engine's own bookkeeping schemas. Engines with no such listing, or a shell
  that cannot answer, fall back to the single name the panel knows rather than
  becoming a dead end.

  Restoring already handled this: every backup records its own database in the
  action history, so the restore picker names it and puts each file back where it
  belongs.

## [0.54.1] — 2026-07-20

### Fixed

- **The arrows work again while a filter is being typed.** Reported from real
  use: type `/mysql`, watch the table narrow to what you wanted, reach for ↓ —
  and nothing happened. The filter had focus and swallowed every navigation key,
  with no hint that Enter was required first. ↑↓, PageUp/PageDown and Home/End
  now move the selection while you keep typing, and the filter bar says so.
  `j`/`k` deliberately still type: a filter is text.

  Each screen used to carry its own copy of "which table, how many rows" in a
  fallback arm; the filter would have been a sixth. They are now one definition,
  so the filter cannot navigate a different list than the screen it sits on.

### Corrected

- The v0.54.0 note claiming a database container **stops answering after a
  restore** was overstated. A controlled run since — restore on a live host,
  probing the restored container and an unrelated one side by side at 10, 30, 60,
  120 and 180 seconds — found the container responsive throughout, never
  restarted, and the data correctly restored. The unresponsiveness was seen on
  one host after several restores in a row and is not a general consequence of
  restoring. It stays recorded in the brief as an observation, not a rule.

## [0.54.0] — 2026-07-20

### Added

- **Restore a database backup taken on ANOTHER EasyPanel host.** Storage ▸
  *Restore from another server* on any database: pick the other host, and its
  backups are listed for you to restore into the service you are on. Proven
  against two live panels — data written on one host was read back on the other.

  Three things this had to get right, all verified rather than assumed:

  - **Provider ids are per-panel.** The same Cloudflare R2 bucket has a different
    id on each host, so the id recorded with the backup is meaningless on the
    destination. Every file is re-pointed at the destination's own remote
    provider, which is what actually reads the bucket.
  - **A local-disk backup can never travel.** It lives on that host's filesystem,
    so those are left out of the list — and the count is stated ("8 more exist
    there on local disk, unreadable from here") so a short list is never mistaken
    for "there are no backups".
  - **The names need not match.** Backups from every project on the source are
    offered, each showing where it came from, so `shop/db` can be restored into
    `shop-staging/db`. Filtering the source by the destination's own names would
    have shown an empty list and explained nothing.

- **"Backup now" says where the backup is going, before it runs.** With more than
  one storage provider configured, which one is used decides whether the backup
  can EVER be restored onto another host. A remote provider is preferred, and the
  confirmation spells out the consequence either way ("restorable on any host
  sharing it" / "stays on THIS host").

### Note

The full path was verified across two hosts by API, and every step of the TUI
flow was verified on screen — the picker, what it hides and why, the
confirmation, and the restore reporting success. Re-reading the data through the
container shell after that last TUI-triggered restore was NOT possible: the
target container stopped answering (both the database client and the shell
itself) after the restore, on that host. Worth knowing if you restore onto a
database and find it briefly unreachable.

## [0.53.0] — 2026-07-20

### Added

- **Back a database up, and restore it — from the Storage menu of any database
  service.** EasyPanel can schedule backups; what it does not give you is a way
  to take one right now and put one back without hunting for a filename.

  **Backup now** takes an immediate dump. There is no endpoint for that —
  `runDatabaseBackup` only runs a *schedule* — so one is created disabled, run,
  and deleted again, leaving nothing behind in the panel's backup list. Verified
  live: a disabled schedule runs fine, and the panel is left with zero schedules
  afterwards. If the database is not running the panel answers "Invariant
  failed", so the error says what that most likely means.

  **Restore from a backup** lists the backups that actually exist and restores
  the one you pick. Nothing in the API lists backup *files*, but every run
  records an action whose `meta` carries the database, the storage provider and
  the exact path — so the action history IS the file list. Only successful runs
  are offered: a failed backup left a path behind too, and restoring from it
  would mean restoring a file that never finished. The confirmation names the
  file, the database and the service it is going into, because restoring the
  right file into the wrong service is the mistake worth preventing.

  Verified end to end against a live panel: a table with two rows, backed up
  through the menu, dropped, then restored through the picker — both rows back.

- **Storage menu now reflects what a service is.** Backups are a database thing;
  `listDatabaseBackups` answers `[]` for an app rather than an error, so the
  entry used to open an empty box on every service in the panel and explain
  nothing.

### Note

Restoring onto a *different* EasyPanel host is not in this release. It needs both
hosts to share one remote storage provider — a backup on a `local` provider
physically lives on that host's disk — and the panel used here has only Local
Disk configured. Backup schedules are still read-only (viewable, not editable),
and marking several databases to back them up in one go is not wired to the bulk
engine yet.

## [0.52.1] — 2026-07-20

### Fixed

- **Resource limits are no longer offered on a compose service.** `compose` has no
  `updateResources` route — a compose stack sets its limits in its own file — so
  both the menu entry and the `L` key could only ever produce a 404. The key is
  guarded as well as the menu, because the leaf keys stay live even where the menu
  hides an entry, which is the gap that left the Lifecycle menu broken for
  databases until v0.52.0.

### Added

- **`box` services can now edit their Config File (Advanced).** They have an
  `updateAdvanced` route, but the editor was restricted to databases, so the panel
  offered something the tool did not.

These came out of probing every operation this tool sends against every service
type on a live panel; the resulting capability matrix is recorded in
`.github/AGENT_BRIEF.md`. Most assumptions held — `save_env` already routes
databases through `updateAdvanced`, and source, build and auto-deploy were already
app-only. Both changes above concern service types that were not present on the
panel used, so they rest on the route probe rather than on a running service.

## [0.52.0] — 2026-07-20

### Fixed

- **Restart, Stop and Start never worked on a single database — and now they do.**
  The tool built every lifecycle call as `services/{type}/{action}Service`, but
  EasyPanel gives databases none of those routes. Probed against a live panel,
  `services/mysql/restartService` (and stop, start, deploy) answers with the bare
  `{"error":"Not found"}` of an unknown route, not the tRPC-shaped 400 a real
  operation gives for a bad argument. The same holds for mariadb, postgres, mongo
  and redis. A database stops and starts through `enabled` instead, so Restart is
  now `disableService` followed by `enableService`, and its menu entry says
  "Restart (stop, then start)" because that is what it does.

  This is what made **editing a database's Config File look like it did nothing.**
  A config change is only picked up when the process restarts, and nothing in the
  tool could restart a database. Measured on a live MySQL: after saving
  `max_connections = 999`, the file was in the container within seconds while the
  running server still reported 151 — and still did nearly two hours later.
  Cycling `enabled` brought it up with the new value. Verified end to end through
  the TUI: a config saved, Restart pressed, and the new value in effect on a
  process 27 seconds old.

- **Actions that cannot exist are no longer offered.** Deploy and Force rebuild
  are gone from the Lifecycle menu for databases — a database is pulled, never
  built — and for `box` and `wordpress` services, which have no deploy route
  either. Anything that still slips through (a saved shortcut, the palette) is
  refused with the reason rather than sent as a request that could only come back
  404. Bulk actions follow the same rule, so marking a set of databases and
  restarting them works too.

## [0.51.1] — 2026-07-20

### Fixed

- **Cloning or migrating a database replica produced a database that could never
  work.** Reported from real use: a cloned `mysql-r1` looked successful but no
  client could log into it, while a cloned primary was fine. The clone faithfully
  copied the replica's config file — including `super_read_only = ON` — onto a
  brand-new, empty data directory. A fresh MySQL must WRITE before it can serve
  anything: the entrypoint creates the root user, sets its password and creates
  the database, and those directives refuse exactly those writes. Verified on a
  live panel, the entrypoint died with `ERROR 1290 … --super-read-only option so
  it cannot execute this statement`, leaving a database with no root password, no
  user and no schema — while the panel still displayed the credentials it believed
  it had set.

  Worse, a database initialises only ONCE, while its directory is empty. The
  failed boot leaves it non-empty, so correcting the config afterwards does not
  repair the service; it has to be destroyed and remade. The config is therefore
  now held back at clone/migrate time rather than applied, and the reason is
  spelled out: deploy the new database first so it initialises, then copy the
  config across — which is also the only order in which a replica's read-only
  flags mean anything, since a clone carries no data to protect. Only configs that
  would actually block the first boot are held back; every other config file is
  copied exactly as before.

- **A clone threw away the very notes the user had to act on.** The migrate path
  reported them; the clone path discarded them (`Ok(_)`), so a skipped config file
  or an auto-deploy that could not be re-enabled was reported as a plain success.

- **Notes no longer get cut in half by the terminal width.** They were appended to
  the one-line status bar, which truncated the sentence exactly where the reason
  began. Anything that needs acting on now opens in the viewer, wrapped and
  readable, and the status line keeps its ⚠ instead of fading away.

## [0.51.0] — 2026-07-20

### Added

- **Bulk actions: deploy, force rebuild, restart, stop or start many services at
  once.** Services are chosen three ways, all feeding one set: `v` marks the row
  under the cursor — or, on a project header, every service in that project; `V`
  marks everything the current filter shows; `Esc` clears the marks. Marked rows
  carry a ✓ and the count stays in the table title, so a set built across several
  screens never becomes invisible. The action menu then offers the bulk entries at
  the top, each naming its count, and the confirmation lists the services it is
  about to touch — a bulk action is never reachable by the same words as a
  single-service one.

  EasyPanel has no batch endpoint (every candidate route answers 404), so this is
  a client-side fan-out, capped so marking a whole panel can't open one connection
  per service at once. Because each call can fail on its own, **a partial failure
  is reported per service**: any run with a failure opens a list naming what broke
  and why (`✗ project/service — [404] Service not found.`) alongside what
  succeeded, instead of a "9 of 12" that hides which three. Deploys are dispatched
  rather than awaited, as a single deploy already is — their builds outlive any
  proxy timeout.

## [0.50.5] — 2026-07-19

### Fixed

- **Creating a service from a Docker image no longer asks how to build it.** The
  wizard walked every new service through a Build step — install command, build
  command, Nix packages — even when the source was a prebuilt image, which is
  pulled and never built. Verified against a live panel: those settings were
  stored by `createService` and then wiped the moment the image source was set,
  so the page could only ever collect answers destined for the bin. An image
  source now skips the step entirely (four pages instead of five), and the
  settings are no longer sent.

- **A refusal no longer outlives the field it names.** A validation message
  stayed on the form border until the next successful step, so it survived the
  very edit that answered it. Switching the source from Dockerfile to image left
  "Dockerfile is still empty" pinned under a form that no longer had a Dockerfile
  field. Any keystroke now dismisses it; a step that still can't be satisfied
  says so again.

## [0.50.4] — 2026-07-19

### Fixed

- **A database service could inherit environment variables you typed for an app.**
  In the create wizard, fill Environment while the kind is `app`, step back and
  change the kind to `postgres`: the form collapses to a single page — the
  Environment field is gone from the screen — but its value still went into the
  request. Checked against a live server: the API **accepts and stores** it, so
  the result was a database quietly holding env you never meant for it, not an
  error you could see. Environment and Domains now honour the service kind, like
  the Source and Build steps already did.

- **`r` on an action's detail said "Refreshing…" and re-fetched nothing.** An
  action detail is a one-shot snapshot, and refresh had no way to ask for it
  again — so for a *running* deploy, the screen you open to watch the log left it
  frozen at the moment it was first fetched, while the status bar confirmed an
  update that never happened.

### Changed

- **Database backup rows lead with the database name.** They used to lead with a
  25-character id that nothing in this view can act on — there is no run, no
  delete, and no selection here — pushing the only thing that tells two rows
  apart off to the right. The columns are labelled now, as the CLI's already
  were.

## [0.50.3] — 2026-07-19

### Fixed

- **A freshly opened list arrived with a row already selected — and `x` deletes
  the selected row.** Every other viewer field reset when a new list loaded, but
  the selection did not, so leaving Ports on row 5 and opening Mounts on another
  service armed row 5 of that list. The confirmation names the resource and index,
  so it was catchable — but it should never have needed catching.

- **The mouse wheel and `j`/`k` did nothing in ports, mounts and redirects.** They
  moved a scroll offset that the list view does not read, so the keys that work on
  every other table here were silently inert on this one. They move the selection
  now.

- **Menus offered actions the service could not have.** A redis service was shown
  "Redirects" and "Basic auth", and "Source & build" — all three refused a
  keystroke later, in a status line that then faded. Redirects was worse: it
  *opened*, showing an empty list under a footer inviting you to add one, for
  something a redis service cannot have. Those entries now appear only on the
  types that support them, using the same lists the handlers already check.

- Pressing `p`/`b`/`f` on a project header did nothing at all, while the same
  actions reached from the menu said "Select a service first".

## [0.50.2] — 2026-07-19

### Fixed

- **An empty collection highlighted its own "nothing here" message as if it were
  a row.** Turning ports, mounts and redirects into selectable tables last
  release made the placeholder look like a selected item — `› No ports`, sitting
  under a border offering `x delete`. It is a message again, and it now says what
  to do: `No ports yet — press n to add one`.

- **The Domains screen showed a blank box when it had nothing to show.** A filter
  that excludes everything and a genuinely empty list need different responses,
  and neither was distinguishable from the other or from a screen that had failed
  to load. It now says either `No domains yet — press n to add one` or
  `Nothing matches '<query>' — Esc clears the filter`.

## [0.50.1] — 2026-07-19

### Fixed

- **The server list cut the URLs that exist to tell servers apart.** The box was
  sized as a percentage of the screen, so on an 80-column terminal it was 36 wide:
  `https://panel.internal.example.com` rendered as `https://panel.internal.exa`
  and `https://panel-staging.internal.example.com` as `https://panel-staging.i` —
  no ellipsis, both reading as complete and different hosts. Its own title lost
  `x delete` the same way.

  This is the screen you pick a server to **edit or delete** on, and the URL is
  there precisely because the name alone is not enough to be sure which one. The
  box is now sized from its content, a URL that still does not fit ends in `…`,
  and the keys are dropped whole rather than cut — the same rule the form footers
  already used.

## [0.50.0] — 2026-07-19

### Changed

- **A collection is now a list you select in, not a list you index into.** Ports,
  mounts and redirects show a highlighted row: `↑↓` moves it, `x` deletes it,
  `n` adds one. Deleting used to mean pressing the digit printed on the line —
  which capped a collection at ten rows, so a service with a dozen mounts simply
  could not remove the last few.

  It also settles a disagreement between screens. Domains and the server picker
  already used `n` add / `e` edit / `x` delete; the viewer used `a` and digits.
  All three now read the same way, and the selection moves with the same keys as
  every other table here — `PageUp`/`PageDown`, `Home`/`End` included.

  Digits go back to being tab switches on every screen, since nothing in the
  viewer wants them any more.

- The "Press a digit [0-9] to delete that mount" lines are gone from the ports,
  mounts and redirects views. The keys live on the screen's bottom border, which
  is where every other screen puts them.

## [0.49.2] — 2026-07-19

### Fixed

- **`[0-9]` deleted nothing for seven digits out of ten — it threw you onto
  another tab instead.** `1`–`7` were global tab-switch keys, so pressing `2` in
  a Ports viewer to delete the second port jumped to the Hosts screen with the
  port still there, no message, nothing to explain it. The viewer's own border
  advertised `[0-9] delete` the whole time, and since each collection became a
  single screen this is its *only* delete.

  The viewer now owns its digits. `Esc` then the digit still switches tab, the
  same arrangement `←`/`→` already use there.

- **Two silent keys in the viewer now speak.** A digit with no row behind it did
  nothing at all — indistinguishable from a broken key — and so did `a`/`e`/`b`
  in a viewer that does not take them. They now say either that the row is absent
  (naming the `[0]`–`[9]` limit when the list is longer) or what this particular
  screen does accept: `Not here — e edit`.

## [0.49.1] — 2026-07-19

### Fixed

- **A Maintenance row that failed to load looked exactly like one that
  succeeded.** Each row is fetched separately so that one broken endpoint cannot
  empty the tab — but a failure was turned into the string `error: …` and drawn
  in the terminal's ordinary text colour, indistinguishable from the real Docker
  version sitting directly above it. This is the screen that offers three
  irreversible host-wide cleanups, so a value that was never fetched must not
  read like one that was.

  Failures now carry through as a typed result and are drawn bold in the error
  colour, wrapped with a hanging indent so the whole reason stays readable
  instead of being cut at the pane edge. The consequence lines under the
  destructive keys (`[p] prune system — …`) no longer truncate on a narrow
  terminal either.

## [0.49.0] — 2026-07-19

### Changed

- **Each collection is now one screen instead of two or three menu entries.**
  Env had three doors — "View env", "Edit env (partial)" and "Replace entire
  env" — for one screen and one operation. Ports, mounts and redirects each had
  a "View X" and a separate "Add X", even though the viewer already deleted rows
  by digit. Source had "View source & build", "Set source" and "Set build".

  Open the thing, act on it there: **`a`** adds, **`[0-9]`** deletes that row,
  **`e`** edits (env, or the source), **`b`** sets the build. Each screen lists
  its own keys along the bottom border.

  | Menu | Entries before | After |
  |---|---|---|
  | Env | 3 | 1 |
  | Networking | 6 | 4 |
  | Build & source | 5 | 4 |
  | Storage | 3 | 2 |

  The keys are routed through the same handlers the menu used, so there is no
  second code path that can drift from it.

### Fixed

- **Two menu labels were wrong.** Saving env sends the whole string, so "Edit env
  (partial)" was never partial — it replaced everything, exactly like "Replace
  entire env". The only difference was whether `$EDITOR` opened pre-filled or
  blank, which is something you do inside your editor rather than a separate
  feature. The blank-editor mode and its `w` key are gone.

### Removed

- The `w` key (open an empty editor to paste a new env). Use `e` and clear the
  buffer in your editor.

## [0.48.11] — 2026-07-19

### Fixed

- **A dropdown whose search matched nothing closed as if you had chosen
  something.** Enter dismissed it, left the field on its previous value and said
  nothing at all — from the outside, identical to a successful pick. Mistype a
  project name in the clone or migrate form and you would walk away believing you
  had changed it.

  It now stays open. The empty box explains itself — `nothing matches —
  Backspace to widen` — instead of being an unexplained blank, and `Enter select
  · Esc cancel` is on its border, where the dropdown previously showed no keys at
  all.

  This was the sibling of a silent close already fixed in the command palette;
  the same rule now holds in both.

## [0.48.10] — 2026-07-19

### Fixed

- **Filtering the Monitor detached each service from its project.** The rows were
  filtered as a flat list, and a project header rarely contains what you typed —
  so the headers were dropped and the matches left orphaned. Filtering for `w`
  produced two rows both reading `webapp`, in different projects, with nothing to
  tell them apart on a screen where you act on the row you pick. It now filters
  per project, exactly as the Services table already did: a matching service keeps
  its project's header, and a matching project keeps all its services.

- **The Domains table cut hostnames into other, plausible hostnames.** Its three
  columns shrank together, so at 80 characters a source rendered as
  `https://harisenin-net-db-mysql-m` — no ellipsis, nothing to say it had been
  cut. This is the screen with `x delete` on it. Whole columns are now dropped
  instead (the ID first: an opaque cuid nobody types, which at 18% was too narrow
  to show in full anyway), Source is never dropped, and anything that still does
  not fit ends in `…`.

### Internal

- The Monitor's rows and its unfiltered count now come from one pass in
  `App::monitor_table()`. A performance change had given the renderer its own
  inline copy of the filtering, and the two drifted: the copy that decided what
  you saw kept filtering flat, so fixing the other one changed nothing on screen.

## [0.48.9] — 2026-07-19

### Fixed

- **The last rows of the Monitor table could not be reached.** Navigation was
  bounded by the number of raw metric entries, but the table also inserts a
  header row per project — so with 60 metrics across 11 projects it drew 71 rows
  and the cursor stopped at 60. `End` landed in the middle of the list and the
  eleven rows below it were unreachable, with no filter involved at all.

- **`/` on the Monitor's Storage view did nothing.** The rows were built from the
  unfiltered list and the title never showed a count, so the filter was both
  inert and invisible — you could type one and get no hint that it had been
  ignored. It now filters and reads `Storage (6/48) /redis`, like every other
  table here.

### Internal

- Three call sites worked out the Monitor's row count independently and all three
  disagreed with each other. There is now one `App::monitor_rows_shown()`, and
  navigation, mouse hit-testing and filter clamping share it.

## [0.48.8] — 2026-07-19

### Fixed

- **The container terminal and DB shell kept no history at all.** Output that
  scrolled off the top was gone — not out of reach, *discarded*: the emulator was
  created with a scrollback length of zero, so no key could have brought it back.
  Run `SHOW REPLICA STATUS\G` and everything above the last screenful was lost.

  The session now keeps 5,000 lines. **Shift+PageUp / Shift+PageDown** walk
  through them, and the mouse wheel does too; typing snaps back to the live
  prompt, so the keys never go to a shell you cannot see answering them. Both are
  listed in the `?` help for the Terminal screen.

  The bindings are held by the UI rather than forwarded to the shell, because
  nothing downstream could serve them — a shell has no idea what scrolled off its
  own output.

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

[Unreleased]: https://github.com/mrfansi/easypanel-cli/compare/v0.98.8...HEAD
[0.96.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.96.0
[0.95.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.95.0
[0.94.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.94.0
[0.93.5]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.93.5
[0.93.4]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.93.4
[0.93.3]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.93.3
[0.93.2]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.93.2
[0.93.1]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.93.1
[0.93.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.93.0
[0.92.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.92.0
[0.91.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.91.0
[0.90.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.90.0
[0.89.1]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.89.1
[0.89.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.89.0
[0.88.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.88.0
[0.87.7]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.87.7
[0.87.6]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.87.6
[0.87.5]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.87.5
[0.87.4]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.87.4
[0.87.3]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.87.3
[0.87.2]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.87.2
[0.87.1]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.87.1
[0.87.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.87.0
[0.86.1]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.86.1
[0.86.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.86.0
[0.85.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.85.0
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
