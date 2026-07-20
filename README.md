# easypanel-cli

[![CI](https://github.com/mrfansi/easypanel-cli/actions/workflows/ci.yml/badge.svg)](https://github.com/mrfansi/easypanel-cli/actions/workflows/ci.yml)
[![Release](https://img.shields.io/github/v/release/mrfansi/easypanel-cli?sort=semver)](https://github.com/mrfansi/easypanel-cli/releases/latest)
[![License: MIT](https://img.shields.io/github/license/mrfansi/easypanel-cli)](LICENSE)

Manage **many [EasyPanel](https://easypanel.io) hosts from one terminal** — a Rust CLI
and a full-screen TUI.

EasyPanel's web panel shows you one server at a time. This shows you all of them: every
host's CPU, memory and disk side by side, every service across every project in one
searchable table, and every domain on the box — without clicking through a hierarchy.

```
┌ Hosts (3) ──────────────────────────────────────────────────────────────────────────┐
│ Server     Status                    CPU     Memory              Disk               │
│ prod       ok                        5.1 %   14.9 GB / 59.0 GB   127.5 GB / 784.9 GB│
│ staging    ok                        0.4 %   2.1 GB / 16.0 GB    38.2 GB / 200.0 GB │
│ eu-west    DEAD — token expired (401)  -       -                   -                │
└─────────────────────────────────────────────────────────────────────────────────────┘
```

**Things it does that the web panel can't, or can't from one screen:**

- **Clone a service** (`c`) — copy a service's whole config (image/source, build, env,
  resources, mounts, ports, and a database's advanced config file) into a new service, in
  any project. Config only, no data, no deploy. EasyPanel's own panel has no clone. The
  motivating case: standing up a **MySQL replica** without re-entering everything by hand.
- **Migrate a service — or a whole project — to another EasyPanel host.** Pick the
  destination server, name the target project (it's created there if missing), and the
  config goes over: image/source, build, env, mounts, ports, resources, a database's
  advanced config file and credentials, and the domains. Moving between hosts is
  painful in the web panel, which has no export and no import — you retype everything.
  **Config only, never data:** volume contents and database rows live on the origin
  host's disk and the API does not expose them, so plan to move those yourself
  (`mysqldump`, a volume copy). Domains come over pointing at their existing hostnames —
  aim the DNS at the new host when you're ready to cut over.
- **Search one keyword across every service's logs at once** (`g`) — a parallel fan-out
  over all services, grouped by where it hit. "Which service logged this error?" in one
  keystroke.
- **A real shell inside any container** (`t`), embedded right in the pane — colours,
  arrows, tab-completion, resize, all of it, over EasyPanel's own WebSocket.
- **One-key database shell** (`y`) — `mysql`, `psql`, `mongosh` or `redis-cli` already
  logged in with that service's own stored credentials. You never type a password, and
  it never appears in `ps`. Nothing like it in the panel.
- **Crash visibility** — a service whose Swarm replicas are missing shows a pulsing red
  `down`, counted in the title, so "what's broken right now?" is answered at a glance.
- **Deploy visibility** — a service that is building shows `deploying` (and a count in
  the title), so you can see a deploy is still running instead of re-triggering it
  blindly. Immediate rejections surface instead of being swallowed.
- **Dead routing, found for you** — a domain whose destination service has been renamed
  or destroyed still looks perfectly healthy in the panel. Here it is marked `✗` and
  counted in the title. On a real host with 713 domains, exactly one was dead; nobody
  finds that by reading.
- **Bulk-edit every domain at once** (`E` on Domains) — find-and-replace one part of many
  domains in a single pass: the host, the destination service, or a custom destination's
  URLs. Filter to the set you mean, see the whole before → after list, then apply. Moving
  a fleet to a new hostname in the panel means opening each domain's dialog in turn.
- **Uptime checks for the domains YOU pick** (`w` on a domain, then the **Uptime**
  tab) — the panel can only tell you what it *intended* to serve; this asks the domain
  itself. Any HTTP method with a body and headers, the status you expect, and a latency
  split into the server thinking (TTFB) and the whole response. Only what you enrol is
  watched, never all of them.
- **Global search / command palette** (`:`) — jump to any service or tab by typing, and
  run any action on the selected row from the same box (`deploy karir`, `logs api`). It
  knows every service from the moment the app starts, so it works before you have opened
  anything.
- **Mouse and keyboard** — click tabs and rows, right-click for a context menu, scroll,
  hover to highlight; every action also has a key.

## Install

Download a binary from [Releases](https://github.com/mrfansi/easypanel-cli/releases)
(linux-x86_64, darwin-arm64, darwin-x86_64), or build it:

```bash
cargo build --release          # target/release/easypanel
./install.sh                   # or: PREFIX=~/.local/bin ./install.sh
```

### Shell completions

```bash
easypanel completions zsh  > ~/.zfunc/_easypanel        # zsh (ensure ~/.zfunc is in $fpath)
easypanel completions bash > /etc/bash_completion.d/easypanel
easypanel completions fish > ~/.config/fish/completions/easypanel.fish
```

`elvish` and `powershell` also work. With no argument the shell is guessed from
`$SHELL`. `install.sh` installs zsh/bash/fish completions automatically when it can
find the right directory.

### Man page

```bash
easypanel man > ~/.local/share/man/man1/easypanel.1
man easypanel
```

Release tarballs ship `easypanel.1` alongside the binary, and `install.sh` installs it
when it finds a writable `man1` directory.

## Configure

Credentials live in `~/.config/easypanel/servers.json`, mode `0600`.

```bash
easypanel server add prod --url https://panel.example.com --token <TOKEN>
easypanel server add prod            # interactive
easypanel server list
easypanel server use prod            # make default
easypanel server remove prod
```

Get a token from EasyPanel → Settings → API Tokens. The first server becomes the
default; every command accepts `--server <name>`.

On the CLI there is no `server edit` — **editing means `add` with the same name**, which
overwrites the entry and keeps its default flag (handy for token rotation).

From the TUI, press `s`: `n` add, `e` edit, `x` delete. The edit form pre-fills the
**name** and URL; a **blank token means unchanged**. Changing the name **renames** the
server in place, keeping its token, its default flag and its position in the list —
renaming onto a name that already exists is refused rather than merging two hosts into
one entry. That matters because a token cannot be read back from anywhere: before, the
only way to fix a typo in a name was to delete the server and lose its token with it.

**Switching host in the TUI is remembered.** `Enter` on a server in that list makes it
the default, so the next launch comes up on the host you were last working on rather
than silently going back to the old one.

## TUI

Run with no arguments:

```bash
easypanel                 # default server
easypanel --server prod   # a specific host
```

Press **`?`** at any time for every shortcut on the current screen.

You don't have to memorise keys. Related actions live in **grouped menus** — one key opens
a list you arrow through (`e` Env, `o` Networking, `u` Build & source, `m` Storage, `d`
Lifecycle, `t` Shell, `x` Danger) — and **`Space`** opens the full action menu for the
selected row. **`:`** opens a **global search**: type to jump to any service or tab, or to
run an action on the row you're on.

The TUI is also **mouse-driven**: click a tab to switch, click any table row to select it,
**right-click a row for a context menu** of its actions, and scroll to move through tables
or the viewer. (Mouse capture disables the terminal's own text selection — hold **Shift
while dragging** to select/copy.)

| Screen | What it is |
|---|---|
| **Dashboard** | CPU/memory/disk gauges, CPU history, load average, cluster nodes — active server only. |
| **Hosts** | **Every** configured server at once. Each host is fetched on its own thread, so a slow or dead one never blocks the rest; failures show red with the reason instead of failing the table. **`Enter` opens a host's detail** — the full reason an unreachable one is unreachable (the table cell only has room for the first few words), or the complete figures for a healthy one. Columns drop as the terminal narrows rather than shrinking into half-numbers. The one screen the web panel can't replace. |
| **Maintenance** | Docker version, server IP, update availability, plus system prune / image cleanup / builder cleanup — all behind confirmation. |
| **Actions** | Deploy/destroy/login history with status, target, duration, age. The status is coloured: green `done`, yellow `killed` (stopped on purpose), red `error` (failed on its own) — so what went wrong is visible without reading every row. |
| **Monitor** | Five history tiles (CPU, memory, disk, net in/out) plus per-service metrics and storage (`v` switches). |
| **Domains** | Every domain on the host: source → destination (internal service or weighted custom servers), SSL resolver, wildcard. A domain pointing at a service that **no longer exists** is marked `✗` in red and counted in the title — dead routing is invisible among hundreds of live rows. `w` enrols one for uptime checks. |
| **Uptime** | The domains you enrolled, and what they last answered — broken ones first, then the slowest. Shows the status code, TTFB, total, and how a domain compares with its peers. `r` checks them all, `e` edits the request, `x` stops watching. |
| **Services** | **Every project and service in one searchable table** — a project header (with its service count and aggregate metrics) followed by its services, so the hierarchy stays visible without drill-down. A colored status dot reads at a glance: green `active`, yellow `stopped`, gray `disabled`, cyan `deploying` (a build is running), and a pulsing red **`down`** for a crashed/restart-looping service (its Swarm replicas are missing), counted in the title. From a selected service you can do the lot, grouped into menus — logs, terminal & DB shell, deploy/restart/stop/start, clone, env (view/edit/replace/`.env` file), ports, mounts, redirects, domains, resource limits, basic auth, and a database's config file. Selecting a project header targets the project and opens its own menu (`Space`): migrate the whole project to another host, new service, new project, destroy project. |
| **Viewer** | Scrollable pane for logs, env, ports, mounts, redirects, backups, source & build. Credentials are masked — a field whose name reads like a secret shows as `••••••••` rather than in the clear. Reached from a service; `Esc` goes back. In the ports/mounts/redirects view, a digit key deletes that row. **Logs tail live** — the pane sticks to the newest line and new output appears as it happens; scrolling up pauses the follow (the title says so) and `End` resumes it. Long lines are not wrapped — `←→` scroll sideways, and the pane shows which column you are on. |

### Key bindings

**Anywhere**

| Key | Action |
|---|---|
| `?` | every shortcut for the current screen |
| `:` | **global search** — jump to any service or tab, or run an action on the selected row |
| `1`–`8`, `Tab`, `←`/`→` | switch tabs (`2` = Hosts, `8` = Uptime) |
| `/` | filter (Services, Domains, Actions, Monitor) · `Esc` clears |
| `s` | server list: `Enter` switch · `n` add · `e` edit (name, URL, token) · `x` delete |
| `r` · `q` | refresh · quit (`Esc` **cancels**, it does not quit) |

**Services — one key opens a menu of related actions**

| Key | Opens |
|---|---|
| `Space` / right-click | the full action menu for the selected row |
| `e` | **Env** — the service's env (`e` there edits it in `$EDITOR`) and the **project env** shared by every service in the project |
| `o` | **Networking** — domains · ports · redirects · basic auth |
| `u` | **Build & source** — source · build · auto deploy · resource limits · a database's **config file** |
| `m` | **Storage** — mounts · backups |
| `d` | **Lifecycle** — deploy · force rebuild (no cache) · restart · stop · start (each confirmed) |
| `t` | **Shell** — container shell · DB shell |
| `x` | **Danger** — delete service · delete project |

Inside a menu: `↑↓` select · `→` enter a submenu · `←` back · `Enter` run · `Esc` close.

**Services — direct keys**

| Key | Action |
|---|---|
| `Enter` | logs for the selected service (**live**; `End` re-follows) · on **Actions**, the action's detail + deploy log |
| `y` | **DB shell** — `mysql`/`psql`/`mongosh`/`redis-cli`, already logged in |
| `g` | **search a keyword across every service's logs at once** |
| `c` | **clone the service's config into a new service** (any project) |
| `Space` | open the action menu for the selected row (a service, or a project header) |
| `p` · `b` | view ports · backups |
| `n` · `N` | new service · new project |

**Each collection is one screen.** Open the env, the ports, the mounts, the redirects or
the source, and act on it from there: `↑↓` selects a row, `n` adds, `x` deletes the
selected one, `e` edits (and `b` sets the build on the source screen). The screen lists
its own keys along the bottom. There is no separate "Add port" or "Replace env" entry any
more — viewing a thing and changing it are the same place, and `n`/`e`/`x` mean the same
here as they do on Domains and in the server list.

The pre-menu keys still work if you have the muscle memory: `E` `.` (edit env, toggle
`.env` file), `P` `M` `F` (add port, mount, redirect), `f` (view redirects), `U` `B` `A`
`L` `H` (source, build, auto deploy, resource limits, basic auth), `R` `S` `T` (restart,
stop, start), `X` (delete project).

**Viewer** — `↑↓`/`PgUp`/`PgDn` scroll · `←→` scroll sideways (lines are not wrapped) ·
`Home` first line and left edge · `End` re-follow the log tail · `[0]`–`[9]` deletes that
row (ports/mounts/redirects) · `Esc` back.
**Domains** — `n` new · `e` edit · `E` bulk edit · `x` delete · `P` set primary.

`E` rewrites one part — the host, the destination service, or a custom
destination URL — across every domain currently on screen, so `/` narrows the
set first. It is a plain find-and-replace, not a regex, and it shows the full
before → after list for approval before anything is sent.
**Terminal** — `Ctrl-Q` to leave (or type `exit`).

**Dockerfile sources open in `$EDITOR`.** Pick `dockerfile` as the source and the field
shows how many lines it holds; `Space` opens the content in `$VISUAL`/`$EDITOR` (the
same hand-off `E` uses for env), and the form takes back the terminal when you quit.
A Dockerfile is not a single-line value, and `updateSourceDockerfile` takes its contents
inline rather than a path — so pretending otherwise would send one long line that never
builds.

### Project environment

A project can hold variables that **every service in it receives**, and `e` → "Project
env" opens them in your editor. The entry says which project it belongs to and how many
services that is, because it is one level up from the service env sitting above it in the
same menu.

Saving is not the whole story, and the tool says so: **running containers keep the old
values until they are deployed again.** Rather than reporting "saved" and leaving you to
discover that later, it names how many services now run stale values and offers to deploy
them — counting only the types that can be deployed at all, since a database or a
WordPress has no build step.


## Uptime checks

A domain in the panel is a routing rule, not a promise. It can point at a service that
was renamed, a port nothing listens on, or a container that stopped — and the panel will
keep showing the rule as if it were fine. The **Uptime** tab (`8`) asks the domains
themselves.

**You choose what is watched, and what it is checked with.** Put the cursor on a domain
(`6`) and press `w`: a form opens for the method, body, headers, timeout and expected
status, and nothing is stored until you save it. Pressing `w` on a domain you already
watch reopens that same form to edit it; `x` on the Uptime tab stops watching. On a host with 700 domains most are aliases and parked names, and a list
that watches everything is a list nobody reads — so the watchlist is deliberately short
and deliberately yours. It is stored per server in
`~/.config/easypanel/checks.json`, mode `0600` (a check may carry an `Authorization`
header).

`r` checks them all at once, eight at a time. Each row shows:

| Column | What it means |
|---|---|
| ● | green = answering as expected · orange = answered, but not with the status you want · red = no answer at all · grey = not checked yet |
| Code | the status it actually returned |
| TTFB | the wait until the response head arrived — **the server thinking** |
| Total | including the body coming down the wire |
| Note | how it compares with its peers, or why it failed |

**Redirects are not followed and count as working.** The question is whether *this*
domain and path answer; following the hop would report on a different URL and time it
too. An http→https jump, a canonical host and a login wall all answer with a 3xx, and
calling those "down" is the false alarm that teaches people to ignore alarms.

`e` on the Uptime tab opens the same form: **any method with a body and headers** (`Name: value`, one per
line, the shape you already paste from curl), a timeout, and the status you *expect* —
an API that correctly answers `401` to an unauthenticated probe is working, and a `200`
from it would be the alarming answer.

### Reading the latency

A single number is close to meaningless: 500 ms is only slow relative to something. Two
comparisons are built in, and neither needs any stored history.

**The split.** A high TTFB with a fast finish is a slow application. A fast start with a
slow finish is a big payload or a slow link. One combined figure tells you there is a
problem; the split tells you where.

**The peers.** Checking the whole watchlist at once gives a median to judge against, so
the Note column can say "3.2× slower" — the same network, the same machine, the same
moment. The median, not the mean: one timeout would drag a mean past everything that is
genuinely slow.

Checks run **from your machine**, so they measure what a visitor experiences — DNS, CDN
and your own link included.

### Your editor

Env files, Dockerfiles and database config files open in your own editor. The temp file
it hands over is created readable by you alone (`0600`) — on a shared machine `/tmp` is
world-readable, and a service's env is the densest collection of secrets it has. The first
of `$EASYPANEL_EDITOR`, `$VISUAL`, `$EDITOR` that is actually installed wins, falling
back to `vi` then `nano`. `EASYPANEL_EDITOR` exists so you can use a terminal editor
here without changing the editor the rest of your machine uses.

**GUI editors work — including VS Code, Cursor, Zed, Sublime and the JetBrains IDEs.**
They need a flag to block until you close the file, and it's added for you:

```bash
export EDITOR=code          # runs as: code --wait <file>
export EDITOR="code -w"     # already correct, left alone
export EDITOR=nvim          # terminal editors block anyway, untouched
```

Without that flag these editors hand the file to an already-open window and exit
immediately — the TUI would read the file back before you typed anything, see no
change, and discard your edit. While a GUI editor is open the terminal says what it's
waiting for, so the blank screen doesn't look like a hang.

If your `code` is a shell alias rather than a real command (common on macOS), point
`$EDITOR` at the binary itself — in VS Code, run **Shell Command: Install 'code'
command in PATH** from the command palette.

**`n` opens a creation wizard that follows the EasyPanel dashboard's flow:**
**Basics → Source → Build → Environment → Domains.** `Enter` advances a step, `Esc` goes
back, and the title shows where you are (`2/5 Source`). A database is a single step —
it has no source or build to configure. Every field is optional except the name; leave
source empty for a bare app you'll configure later, and the *Create .env file* toggle in
Environment writes the vars as a `.env` file (the dashboard's "Create env file").

Creating the service does **not** deploy it. It appears in the table in about a second,
and you press `d` to deploy when you're ready — the same order the dashboard uses.
(Applying a source inline at creation would trigger a ~100-second build-and-deploy that
can fail before the row even shows; the source is applied as a separate, config-only
call instead.)

Logs are not a snapshot. EasyPanel has no log streaming endpoint — there is exactly one
SSE route on the whole API and it is for the Actions list — so the tail polls
`queryServiceLogs` every two seconds with `start` set past the newest line already
shown, which fetches only what is new rather than re-pulling 200 lines. It runs on the
metrics lane, so it never queues behind a key you pressed.

The Services table has a **Repl** column: how many replicas the service runs. While
Swarm's live count matches the target it is just the number; when they differ it
shows both — `0/1` for replicas that never came up, `1/3` mid-rollout — which is
exactly when the number matters. It falls back to the configured `deploy.replicas`,
and shows `-` for a service with no deploy block.

**The table adapts to the terminal.** Under ~120 columns the four metric columns
(CPU, memory, net in/out) are dropped rather than squeezed into unreadable slivers;
identity, status, replicas, source and auto deploy always survive. The numbers are on
the Monitor tab either way.

The Services table has an **Auto** column: `✓` auto deploy on, `✗` off, `-` not
applicable. Only GitHub sources have auto deploy at all — it works by creating a
webhook — so a database or an image-sourced app shows `-`, never `✗`. `✗` would claim
it was turned off, when in truth it was never available.

Filters match the text you **see**, so `mysql` finds it via service name, project name,
type, or source. The title shows `(matched/total)`, and filters clear when you switch
tabs — a hidden filter makes missing rows look like missing data. Every keystroke puts
the view back on the **first match**, so narrowing a long list always shows you results
rather than leaving you parked wherever you had scrolled to. The arrows work while you
are still typing.

Creating a database service asks for what the panel asks for — database name, user,
password, root password, image — with the fields swapping to match the type you pick.
All optional: **leave one empty and the server generates it** (random password, database
named after the project, official latest image), exactly like the web panel. Empty
fields are omitted from the request rather than sent as `""`, because those are not the
same thing: `""` creates a MySQL with no database, no user and no password.

Choice fields (project, service, repo, branch, type) open a searchable dropdown rather
than free text, so a typo can't point a domain at a service that doesn't exist. Repo and
branch lists come from live GitHub data via the panel.

Network runs on worker threads, so the UI never freezes on a slow request. Metrics poll
on a separate lane and can't block your keystrokes.

## CLI

```bash
easypanel project list|create|inspect|destroy

# Lifecycle
easypanel service create|deploy|restart|start|stop|destroy <project> <service> [--type app]
easypanel service deploy <project> <service> --force   # rebuild, ignoring the layer cache
easypanel service logs <project> <service> [--limit 100]

# Environment (set-env replaces everything; reads --file or stdin)
easypanel service env|set-env <project> <service> [--file .env]

# Ports, mounts, domains
easypanel service ports|mounts|domains <project> <service>
easypanel service port-add    <project> <service> --published 8080 --target 80
easypanel service mount-add   <project> <service> --kind volume --name data --mount-path /data
easypanel domain list|delete|set-primary

# Databases & backups
easypanel service databases|backups|volume-backups <project> <service>
easypanel backup db-run|db-delete|volume-run|volume-delete <id>
easypanel backup providers
easypanel backup db-restore --project P --service S --database D --path <path> [--yes]

# Maintenance (active server)
easypanel maintenance info|prune|cleanup-images|cleanup-builder [--yes]

# Monitoring
easypanel stats                      # CPU/mem/disk/load
easypanel monitor services|storage
easypanel node list
easypanel action list [--limit 25]
easypanel certificate list|remove
easypanel notification list|delete
```

`--type` defaults to `app`; other types (mysql, postgres, redis, mongo, mariadb,
wordpress, compose) match your EasyPanel services.

### Scripting with `--json`

Add `--json` to any read-only command and it prints **EasyPanel's own JSON** instead of
a table — the exact response the server sent, not a schema this tool invented, so it
tracks the API rather than drifting from it. An empty result is `[]`, not a human
message, so `jq` never chokes.

```bash
easypanel project list --json | jq -r '.[].name'
easypanel stats --json | jq '.memory'
easypanel monitor services --json | jq 'map(select(.cpu > 50))'
easypanel service ports web api --json
```

Works on `project list`/`inspect`, `stats`, `node list`, `monitor services`/`storage`,
`domain list`, `service ports`/`mounts`/`domains`/`databases`/`backups`/`volume-backups`,
`action list`, `certificate list`, and `notification list`. Mutating commands ignore it.

## Known limits

Stated plainly, because a README that promises what the code doesn't do is a bug.

- **Database restore is CLI-only, deliberately.** `restoreDatabaseBackup` needs the
  backup file's `path`, and the API has **no endpoint that lists backup files** — only
  schedules. A TUI form with a path you have to guess, for an operation that overwrites
  a live database, is a trap. The CLI makes you supply a path you actually know, and
  confirms first.
- **One of the panel's five source types is missing: Upload.** Github, Git, Docker
  Image and Dockerfile all work. Upload needs to hand a code archive to the server, but
  the API models it as a server-side `archivePath` — a path that must already exist on
  the box — so there is no file transfer to implement against. Tracked in
  `.github/AGENT_BRIEF.md`.
- **Middlewares aren't editable.** The group has 14 Traefik-style types. They're
  *always preserved* when you edit a domain, so nothing is lost — but there's no editor
  yet. Open an issue if you use them.
- **Docker Events isn't available.** It's a live WebSocket stream, not part of the
  documented REST API.
- **`updateSourceGithub` resets `autoDeploy` server-side.** Not this tool's bug, but
  worth knowing: it silently sets `autoDeploy` to `false` on every successful call, so
  merely changing a branch disables auto-deploy. This CLI restores the value afterwards
  and exposes it as a toggle. Any client that doesn't will quietly break your deploys.

## How it talks to EasyPanel

tRPC-style: `POST {url}/api/rpc/{group}/{op}`, header `Authorization: Bearer <token>`,
body **always** `{"json": <input>}` (`null` when there are no parameters — an empty body
returns 400). Responses wrap the payload in `json` — **and so do errors**, which is why
reading only the top level swallowed every server message for months.
`easypanel-api.json` documents all 374 endpoints; a new command is usually one
`EasypanelClient::call(group, op, input)`.

Metrics come from the Prometheus-backed `metrics` group, not `monitorOld`: ~0.3 s versus
~2.3 s, and one call returns current values, history, rates, and load average.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md) — especially the recurring bug classes, and why a
test that encodes a wrong assumption is worse than no test.

```bash
cargo fmt --check && cargo clippy --all-targets -- -D warnings && cargo test
```

## Licence

MIT — see [LICENSE](LICENSE).
