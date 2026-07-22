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

- `parse_envelope::<T>(body) -> Result<T>` — Cloudflare wraps every response in
  `{ success, errors:[{code,message}], messages, result, result_info }`. On
  `success:false` surface `errors[0].message` (e.g. *"Record already exists."*), not
  the HTTP status. One helper, used by every call.
- `record_body(form) -> serde_json::Value` — build the create/update JSON from typed
  fields; omit `priority` unless the type needs it; `proxied` only for proxyable types.
- `resolve_zone(zones, needle) -> Option<Zone>` — accept a zone **name** (`example.com`)
  or an **id**; prefer an exact name match, fall back to id equality.
- `valid_record_type(&str)`, `proxyable(type)` — small guards used by both CLI and TUI.

### Infrastructure — `CloudflareClient`

Separate from `EasypanelClient`. `reqwest::blocking`, base
`https://api.cloudflare.com/client/v4`, header `Authorization: Bearer <token>`,
`Content-Type: application/json`.

Methods (each returns a domain type via `parse_envelope`):

```
list_zones()                         GET  /zones            (paginated: per_page=50, follow result_info.total_pages)
create_zone(name, account_id)        POST /zones
delete_zone(zone_id)                 DELETE /zones/{id}
list_records(zone_id)                GET  /zones/{id}/dns_records   (paginated)
create_record(zone_id, body)         POST /zones/{id}/dns_records
update_record(zone_id, rec_id, body) PUT  /zones/{id}/dns_records/{rid}
delete_record(zone_id, rec_id)       DELETE /zones/{id}/dns_records/{rid}
```

Pagination matters: an account can hold hundreds of zones and a zone hundreds of
records. Both list calls follow `result_info.total_pages` (the same discipline the
EasyPanel 713-domain host forced on the rest of the tool).

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

easypanel cf record list   <zone>        [--account NAME]
easypanel cf record add    <zone> --type A --name x --content 1.2.3.4
                                   [--ttl N] [--proxied] [--priority N] [--account NAME]
easypanel cf record edit   <zone> <record-id> [--content …] [--proxied true|false]
                                   [--ttl N] [--name …] [--priority N] [--account NAME]
easypanel cf record delete <zone> <record-id>              [--account NAME]
```

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
  - `Zones` table → **Enter** → `Records` table for that zone.
  - Record add/edit/delete and zone add/delete reuse the existing **Form**, context
    **menu**, **viewer**, and confirmation-dialog machinery. No parallel widgets.
  - **Esc** in Records → Zones; **Esc** in Zones (the root) → back to EasyPanel.
  - **`a`** in the Cloudflare workspace opens an **account picker** (list of stored CF
    accounts) — the isolated analogue of `s` for EasyPanel servers. Switching account
    re-lists zones. With a single account it just shows which one is active.
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
CfReq: Zones, CreateZone{name}, DeleteZone{id}, Records{zone_id},
       CreateRecord{zone_id, body}, UpdateRecord{zone_id, id, body}, DeleteRecord{zone_id, id}
CfResp: Zones(Vec<Zone>), Records{zone_id, Vec<Record>}, Done(msg), Err(msg)
```

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
- No bulk record import/export in v1.
- No caching of zones/records to disk — always live from the API.
- The Switch menu stays two-item; no plugin framework for "workspaces".
