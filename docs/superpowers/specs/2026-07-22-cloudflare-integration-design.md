# Cloudflare integration — design

Date: 2026-07-22
Status: approved-in-principle, pending spec review

## Purpose

Add a **Cloudflare** capability to easypanel-cli that is deliberately *outside*
EasyPanel's scope: manage a Cloudflare account's zones and DNS records from the same
CLI + TUI the operator already uses for their EasyPanel hosts. This is not a wrapper
around anything EasyPanel exposes — it talks straight to the Cloudflare API.

Three operator jobs, from the original request:

1. **Manage Cloudflare accounts** — store one or more scoped API tokens, independent of
   any EasyPanel server (an operator may hold several Cloudflare accounts). One is the
   active/default.
2. **Manage zones** — list, add, and delete zones on the active account.
3. **Manage records within a zone** — full CRUD over DNS records.

## Decisions (locked with the owner)

| Decision | Choice |
|---|---|
| Auth | Cloudflare **API Token** (scoped), `Authorization: Bearer`. No Global API Key. |
| Zone ops | Full CRUD: list, add, delete. |
| Record ops | Full CRUD: list, add, edit, delete. |
| Surface | CLI **and** TUI in the same release. |
| TUI placement | **Isolated switch-mode**, NOT a 9th EasyPanel tab. |
| CF account scope | **Standalone, isolated** — its own account store, *multiple* accounts, one active/default. NOT tied to any EasyPanel server. |
| Verification | End-to-end CRUD against a **throwaway zone** whose token the owner supplies via `! export CF_TEST_TOKEN=…`, then cleaned up. |
| Switch key | **`W`** (shift+w) opens the workspace switch menu. Verified free — `w` is taken, `W` is not. |

## Bounded context

Cloudflare is a **separate bounded context** from EasyPanel. Nothing in the EasyPanel
domain, client, config, or tab bar changes behaviour. Isolation is the point: the two
share only the TUI event loop that hosts them and the config *directory* they keep
separate files in. In particular the `Server` struct is **not** touched — Cloudflare
accounts live in their own store because they are independent of EasyPanel servers.

### Domain model — `src/cloudflare.rs`

Pure types + pure functions (all unit-testable, no I/O):

```rust
pub struct CloudflareAccount {
    pub name: String,               // user-chosen label, e.g. "personal", "harisenin"
    pub api_token: String,
    pub account_id: Option<String>, // needed only for `zone add`
    #[serde(default)] pub default: bool,
}

pub struct Zone   { pub id: String, pub name: String, pub status: String }
pub struct Record {
    pub id: String,
    pub r#type: String,        // A, AAAA, CNAME, TXT, MX, NS, SRV, …
    pub name: String,          // fully-qualified (api returns it that way)
    pub content: String,
    pub ttl: u32,              // 1 = "automatic"
    pub proxied: bool,         // orange-cloud; only meaningful for A/AAAA/CNAME
    pub priority: Option<u16>, // MX/SRV only
}
```

Pure functions:

- `parse_envelope::<T>(body) -> Result<T>` — Cloudflare wraps every v4 response in
  `{ success, result, result_info{count,page,per_page,total_count,total_pages},
  errors:[{code, message, documentation_url?, source?}], messages }`. On `success:false`
  surface `errors[0].message` (e.g. *"Record already exists."*), not the HTTP status. One
  helper, used by every call. (Note: Cloudflare's *edge* error pages use a different
  RFC-9457 shape — that is NOT the v4 API envelope and must not be conflated with it.)
- `record_body(form) -> serde_json::Value` — build the CREATE JSON from typed fields.
  Required by CF: `name`, `type`, `ttl` (verified). `ttl = 1` means "automatic"; any other
  value is seconds (60–86400). `proxied` only for A/AAAA/CNAME; `priority` only for MX/SRV.
  **v1 record-type scope**: the flat-`content` types — A, AAAA, CNAME, TXT, NS — plus MX
  (which adds `priority`). The `data`-object types (SRV, CAA, LOC, URI, …) need a
  structured `data{}` body and are deferred to a follow-up; `valid_record_type` rejects
  them with a clear "not supported yet" rather than sending a malformed body.
- `resolve_zone(zones, needle) -> Option<Zone>` — accept a zone **name** (`example.com`)
  or an **id**; prefer an exact name match, fall back to id equality.
- `valid_record_type(&str)`, `proxyable(type)` — small guards used by both CLI and TUI.
- `select_records(records, selector) -> Vec<&Record>` — resolve a bulk selector
  (explicit ids and/or `where-content` / `where-type` / `where-name`) to the concrete
  set of records it matches. Pure, so the CLI can print "these N will change" before any
  write, and it is unit-tested independently of the network.
- `RecordPatch { content?, proxied?, ttl?, priority? }` + `apply_patch(record, patch)` —
  a partial field-change. `record_body` builds the full create body; `apply_patch` builds
  the update body from only the fields the user set, for both single edit and bulk.

### Infrastructure — `CloudflareClient`

Separate from `EasypanelClient`. `reqwest::blocking`, base
`https://api.cloudflare.com/client/v4`, header `Authorization: Bearer <token>`,
`Content-Type: application/json`.

Methods (each returns a domain type via `parse_envelope`):

```
list_zones()                         GET  /zones            (paginated: per_page=50, follow result_info.total_pages)
create_zone(name, account_id)        POST /zones
delete_zone(zone_id)                 DELETE /zones/{id}
list_records(zone_id, filter)        GET  /zones/{id}/dns_records   (paginated; server-side filter)
create_record(zone_id, body)         POST  /zones/{id}/dns_records
patch_record(zone_id, rec_id, patch) PATCH /zones/{id}/dns_records/{rid}   (partial — send only changed fields)
delete_record(zone_id, rec_id)       DELETE /zones/{id}/dns_records/{rid}
```

**PATCH, not PUT, for edits** (verified against the official docs — see the API note at
the end). Cloudflare exposes both: `PATCH .../dns_records/{id}` is "Update" (partial), and
`PUT` is "Overwrite" (full, and omitted optional fields are not preserved). Since both the
single edit and the bulk `apply_patch` change only the fields the user named, they use
PATCH — a PUT would silently wipe every field the user didn't resend.

Pagination matters: an account can hold hundreds of zones and **a single zone can hold
thousands of records**. Both list calls follow `result_info.total_pages` (the same
discipline the EasyPanel 713-domain host forced on the rest of the tool).

**Filtering — pushed to the API when it can be.** Cloudflare's `dns_records` endpoint
accepts `type`, `name` (with a `contains` match mode), and `content` query params, so
`list_records` takes an optional `RecordFilter { type?, name?, content? }` and sends it
as query params — the server returns only the matching page(s), which is what makes a
thousand-record zone usable instead of fetching everything and scanning locally. The
interactive TUI filter (below) narrows the already-loaded page client-side, the same
`/`-filter every other TUI table uses; a "refine on the server" action re-issues
`list_records` with the filter for zones too large to hold a full page comfortably.

### Config — a standalone `CloudflareConfig` store

Cloudflare accounts live in their **own** file, `~/.config/easypanel/cloudflare.json`
(next to `servers.json` and `checks.json`, `0o600`). The `Server` struct is untouched.

`CloudflareConfig` mirrors `ServerConfig` exactly — same shape, same hard-won
corrupt-file guard:

```
list()                 -> Vec<CloudflareAccount>     // read path, empty on corrupt
try_all()              -> Result<Vec<…>>             // write path, errors on corrupt
add(account)                                         // add or replace by name
remove(name)
set_default(name)
default() / by_name(name) -> Option<CloudflareAccount>
```

`try_all` (not `list`) on every write, for the identical reason `ServerConfig` does it:
add/remove/set-default read-modify-write the whole file, so treating a corrupt file as
empty would silently delete every stored CF token — unrecoverable. A *missing* file is
empty; a *corrupt* file stops the write. Written `0o600`, same as the token files beside
it. This is deliberately a copy of the `ServerConfig`/`Watchlist` pattern already proven
in this codebase, not a new abstraction over both.

## CLI surface

Cloudflare accounts are managed on their own, independent of `easypanel server …`:

```
easypanel cf account add <name> [--account-id ID] [--token T]
                        add/replace a CF account by label. Without --token, prompt for
                        it WITHOUT echo (dialoguer password). Never printed back.
                        --account-id is needed only for `zone add`. First account added
                        becomes the default.
easypanel cf account list                  # labels, which is default, masked token
easypanel cf account use    <name>         # set the default account
easypanel cf account delete <name>
```

Zone/record commands run against the default account, or `--account <name>` to pick one:

```
easypanel cf zone list                   [--account NAME]
easypanel cf zone add    <name>          [--account NAME]
easypanel cf zone delete <zone>          [--account NAME]   # destructive; typed-name confirm

easypanel cf record list   <zone>        [--type A] [--name substr] [--content substr] [--account NAME]
easypanel cf record add    <zone> --type A --name x --content 1.2.3.4
                                   [--ttl N] [--proxied] [--priority N] [--account NAME]
easypanel cf record edit   <zone> <record-id> [--content …] [--proxied true|false]
                                   [--ttl N] [--name …] [--priority N] [--account NAME]
easypanel cf record delete <zone> <record-id> [<record-id> …]   [--account NAME]
```

**Bulk operations** (the point of the feature for a migration — repoint many records at
once). One verb, `cf record set`, applies the same field change to a *selection*:

```
easypanel cf record set <zone> [SELECTOR] [FIELD-CHANGES] [--account NAME] [--yes]

  SELECTOR (choose the records; combinable):
    <record-id> [<record-id> …]     explicit ids
    --where-content <value>         every record whose content == value  (the repoint case)
    --where-type <A|CNAME|…>        narrow to a record type
    --where-name <substr>           narrow to names containing substr

  FIELD-CHANGES (what to set on each selected record):
    --content <new>   --proxied <true|false>   --ttl <N>   --priority <N>
```

- Canonical migration: `cf record set example.com --where-content 203.0.113.10 --content 198.51.100.20`
  repoints every record on the old IP to the new one in a single command.
- `cf record set` always prints the matched records and asks to confirm before writing
  (skip with `--yes` for scripts). It reports per-record success/failure and exits
  non-zero if any failed — never a silent partial.
- `cf record delete` accepts several ids for bulk delete; a bulk delete by selector is
  `cf record set … ` territory only for edits, so destructive bulk stays explicit-ids
  (you must name what you delete).

- `<zone>` is a domain name or a zone id (`resolve_zone`).
- `--account NAME` selects the CF account; default is the one marked default. This is a
  `cf`-local flag, unrelated to the global `--server` (which stays EasyPanel-only).
- The global `--json` flag applies to the read commands (prints the raw Cloudflare
  `result`), same contract as the EasyPanel read commands.
- Running a zone/record command with **no** accounts configured errors with a one-line
  "run `easypanel cf account add <name>` first", not a bare 401.

## TUI surface — isolated switch-mode

The app gains a top-level mode, orthogonal to the EasyPanel `Screen`:

```rust
enum Workspace { Easypanel, Cloudflare }
```

- **`W`** opens a small **Switch** menu (`EasyPanel` / `Cloudflare`). Selecting a
  workspace swaps the whole view. The menu is a normal picker (reuses the menu
  machinery); two entries today, extensible if another integration ever lands.
- In **Cloudflare** workspace the EasyPanel tab bar is not drawn; instead a
  Cloudflare-orange header shows the **active CF account label** + two internal screens:
  - `Zones` table → **Enter** → `Records` table for that zone. Both are **filterable**
    with the existing `/` filter (essential — a zone can list thousands of records);
    Zones and Records join `filterable()` exactly like the EasyPanel data tables, so the
    filter, its title count, and Esc-to-clear all work with no new machinery. On the
    Records screen the filter narrows the loaded page instantly; for a zone larger than
    one page, the filter text also feeds a server-side `list_records` refine so nothing
    off-page is missed.
  - Record add/edit/delete and zone add/delete reuse the existing **Form**, context
    **menu**, **viewer**, and confirmation-dialog machinery. No parallel widgets.
  - **Esc** in Records → Zones; **Esc** in Zones (the root) → back to EasyPanel.
  - **`a`** in the Cloudflare workspace opens an **account picker** (list of stored CF
    accounts) — the isolated analogue of `s` for EasyPanel servers. Switching account
    re-lists zones. With a single account it just shows which one is active.
  - **Bulk on the Records screen** — mark records with `v`/`V` (the same marking keys as
    the EasyPanel tables), then the action menu offers *"Set content on N marked"*,
    *"Set proxied on N marked"*, *"Set TTL on N marked"*, and *"Delete N marked"*. Each
    opens one form (or a confirm), applies it to every marked record via its own API
    call, and reports per-record pass/fail — the exact pattern EasyPanel's bulk resource
    edit already uses (`bulk_targets()` → worker loop → `BulkDone`). This is the TUI face
    of `cf record set`: mark the rows, change the field once.
- **Colour carries meaning**: the orange accent makes it unmistakable you've left
  EasyPanel. No EasyPanel state (projects, services, servers, the 1–8 keys) is reachable
  or rendered while in the Cloudflare workspace, and vice-versa. The active CF account is
  independent of the active EasyPanel server.
- **No accounts → honest empty state**: entering the Cloudflare workspace with no CF
  accounts stored shows *"No Cloudflare account yet — press `n` to add one"* (opens the
  same add-account form), never a crash or a silent blank.

### Worker messages (mirrors the EasyPanel Req/Resp pattern)

`Req::Cf(CfReq)` / `Resp::Cf(CfResp)` keep the Cloudflare traffic in its own arm so
the EasyPanel worker match is untouched in spirit:

```
CfReq: Zones, CreateZone{name}, DeleteZone{id}, Records{zone_id, filter},   // filter: Option<RecordFilter>
       CreateRecord{zone_id, body}, UpdateRecord{zone_id, id, body}, DeleteRecord{zone_id, id},
       BulkPatchRecords{zone_id, ids, patch},    // PATCH one field-change onto each id
       BulkDeleteRecords{zone_id, ids}
CfResp: Zones(Vec<Zone>), Records{zone_id, Vec<Record>}, Done(msg),
        BulkDone{ok, failed},                    // per-record report, reuses the EasyPanel shape
        Err(msg)
```

The bulk arms loop over `ids` on the worker thread, one API call per record, collecting
`(id, Result)` — a mid-list failure never aborts the rest, and the report names which
records failed and why. `patch` is the same typed field-change the single edit uses, so
there is one body-builder, not two.

The worker builds a `CloudflareClient` from the **active CF account's** stored token per
request (same lifetime model as the EasyPanel client). The active account is TUI state,
seeded from the default account, changed by the `a` account picker.

## Error handling

- Every Cloudflare error surfaces `errors[0].message` via `parse_envelope` — the CF
  API is good about human-readable messages ("Record already exists", "Invalid TTL").
- Destructive actions (**zone delete**, **record delete**) always confirm and name the
  target. `zone delete` requires **typing the zone name** to proceed — deleting a zone
  removes every DNS record in it and cannot be undone.
- A missing/expired token surfaces as a clear "token rejected — re-add with `cf account add`",
  distinguished from an empty result (the empty-vs-failed lesson from the EasyPanel
  screens applies here too).

## Testing

**Unit (no network — the bulk):**
- `parse_envelope`: success unwraps `result`; `success:false` yields `errors[0].message`;
  a shape with no errors array still fails cleanly.
- `record_body`: A record omits `priority`; MX includes it; `proxied` dropped for TXT.
- `resolve_zone`: name match, id match, no match; name preferred over a coincidental id.
- `RecordFilter` → query params, using the EXACT CF operator-key syntax (verified): a
  substring filter is `name.contains=<v>` / `content.contains=<v>` (siblings `.exact`,
  `.startswith`, `.endswith`; case-insensitive), while `type=<v>` is a flat key. When more
  than one filter is set, add `match=all` (AND). An empty filter sends no params.
- `select_records`: ids-only; `where-content` matches exact content (the repoint case);
  combined `where-type` + `where-content` intersect; `where-name` substring; no match →
  empty (so the CLI says "0 matched" instead of writing nothing silently).
- `apply_patch`: only the set fields appear in the update body; an all-empty patch is a
  no-op the CLI rejects ("nothing to change"); bulk and single edit produce identical
  bodies for the same patch.
- Config: `CloudflareConfig` add/remove/set_default round-trip; first account added is
  default; a missing `cloudflare.json` reads empty; a corrupt one refuses to write (the
  same guard `ServerConfig` has). `servers.json` is untouched and still parses.
- TUI: the Switch menu toggles `Workspace`; Esc from Zones root returns to EasyPanel;
  the no-accounts empty state renders the add-account prompt, not a blank; the `a` picker
  changes the active account.

**Live (throwaway zone, `CF_TEST_TOKEN`):**
- `cf account add` stores the token; `zone list` returns the throwaway zone; add a record of
  each proxyable + non-proxyable type; edit content + toggle proxied; delete; confirm
  each via a re-list. Then, if the token allows, `zone add`/`zone delete` on a genuinely
  disposable name. Clean up everything created.
- **Bulk**: seed several A records on one IP, `cf record set --where-content OLD --content NEW`,
  confirm every one moved via a re-list; bulk-delete several ids in one call and confirm
  the report. Verify a partial failure (one bad id among good ones) reports per-record and
  still applies the rest.

## Sequencing (one feature, phased plan)

Lands as a single minor release (**v0.83.0**) because clippy runs `-D warnings` and the
crate has no `#[allow(dead_code)]`: an orphan module with no caller fails the build.
Internal order the implementation plan will follow:

1. **Foundation** — `CloudflareConfig` store (`cloudflare.json`) + `src/cloudflare.rs`
   domain types (incl. `CloudflareAccount`) and pure functions, all unit-tested.
   (Compiles because the tests use it.) `Server`/`servers.json` untouched.
2. **Client** — `CloudflareClient` wired to the first real consumer.
3. **CLI** — `easypanel cf …`, verified live against the throwaway zone.
4. **TUI** — `Workspace` mode, Switch menu, Zones/Records screens, reusing forms/menus.

## Non-goals (YAGNI)

- No Global API Key auth — token only.
- No page rules, WAF, workers, analytics, SSL settings — zones + DNS records only.
- Bulk **edit** (`cf record set`) and bulk **delete** are IN. Bulk **import/export** from
  a file (BIND zone files, CSV) is out for v1.
- No caching of zones/records to disk — always live from the API.
- The Switch menu stays two-item; no plugin framework for "workspaces".

## API grounding — verified against the official docs (2026-07-22)

Endpoint shapes above were checked against `developers.cloudflare.com/api` (the current
per-resource/per-method reference), not guessed. Confirmed directly: `Authorization:
Bearer` + base `https://api.cloudflare.com/client/v4/`; the `success/result/result_info/
errors/messages` envelope and error `{code,message,documentation_url?,source?}`;
PATCH=partial "Update" vs PUT=full "Overwrite" for records; the `name.contains=` /
`content.contains=` operator-key filter syntax with `match=all`; create requires
`name`+`type`+`ttl` (`ttl=1`=automatic); the `"Edit zone DNS"` token template =
`Zone>DNS>Edit` + `Zone>Zone>Read`.

**Six details the docs did NOT pin down — resolve by a live probe with `CF_TEST_TOKEN`
BEFORE trusting them in code (a measured 400 beats a doc guess):**

1. **Max `per_page`** for both list endpoints — the docs show only an example `20`. Probe
   `per_page=10000`, read the clamp/400; page defensively until then.
2. Whether a **zone-scoped token can `GET /zones` without `account.id`** (may 403). If it
   does, `zone list` may need the account id or a documented "use the token's account".
3. Whether **`account.id` is truly optional at `POST /zones`** — schema says optional, but
   for a token this is almost certainly required. The spec already treats `--account-id`
   as required for `zone add`; the probe confirms the exact 400 if omitted.
4. The exact **Zone-category permission-group names** (beyond the confirmed DNS-edit
   template) — enumerate via the "List permission groups" API if needed.
5. A directly-quoted **"PATCH sends only changed fields"** sentence — inferred from the
   Update-vs-Overwrite naming; the probe (PATCH one field, re-GET, confirm the others
   survived) makes it certain.
6. The **zone `name` filter operator syntax** — probably the same `.contains` pattern as
   DNS records, not directly confirmed for zones.

These are exactly the kind of scoped-token edge cases the brief warns a mock can encode
wrongly — so they are verified live on the throwaway zone during the CLI phase, not
asserted from a doc page.
