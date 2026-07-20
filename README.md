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
- **Global search / command palette** (`:`) — jump to any service or tab by typing, and
  run any action on the selected row from the same box (`deploy karir`, `logs api`).
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

There is no `server edit` — **editing means `add` with the same name**, which overwrites
the entry and keeps its default flag (handy for token rotation). From the TUI, press
`s`: `n` add, `e` edit (URL pre-filled; **blank token = unchanged**), `x` delete.

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
| **Actions** | Deploy/destroy/login history with status, target, duration, age. |
| **Monitor** | Five history tiles (CPU, memory, disk, net in/out) plus per-service metrics and storage (`v` switches). |
| **Domains** | Every domain on the host: source → destination (internal service or weighted custom servers), SSL resolver, wildcard. |
| **Services** | **Every project and service in one searchable table** — a project header (with its service count and aggregate metrics) followed by its services, so the hierarchy stays visible without drill-down. A colored status dot reads at a glance: green `active`, yellow `stopped`, gray `disabled`, cyan `deploying` (a build is running), and a pulsing red **`down`** for a crashed/restart-looping service (its Swarm replicas are missing), counted in the title. From a selected service you can do the lot, grouped into menus — logs, terminal & DB shell, deploy/restart/stop/start, clone, env (view/edit/replace/`.env` file), ports, mounts, redirects, domains, resource limits, basic auth, and a database's config file. Selecting a project header targets the project and opens its own menu (`Space`): migrate the whole project to another host, new service, new project, destroy project. |
| **Viewer** | Scrollable pane for logs, env, ports, mounts, redirects, backups, source & build. Reached from a service; `Esc` goes back. In the ports/mounts/redirects view, a digit key deletes that row. **Logs tail live** — the pane sticks to the newest line and new output appears as it happens; scrolling up pauses the follow (the title says so) and `End` resumes it. Long lines are not wrapped — `←→` scroll sideways, and the pane shows which column you are on. |

### Key bindings

**Anywhere**

| Key | Action |
|---|---|
| `?` | every shortcut for the current screen |
| `:` | **global search** — jump to any service or tab, or run an action on the selected row |
| `1`–`7`, `Tab`, `←`/`→` | switch tabs (`2` = Hosts) |
| `/` | filter (Services, Domains, Actions, Monitor) · `Esc` clears |
| `s` | server list: `Enter` switch · `n` add · `e` edit · `x` delete |
| `r` · `q` | refresh · quit (`Esc` **cancels**, it does not quit) |

**Services — one key opens a menu of related actions**

| Key | Opens |
|---|---|
| `Space` / right-click | the full action menu for the selected row |
| `e` | **Env** — opens the env; `e` there edits it in `$EDITOR` |
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

### Your editor

Env files, Dockerfiles and database config files open in your own editor. The first
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
tabs — a hidden filter makes missing rows look like missing data.

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
