# Changelog

All notable changes to this project are documented here.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/mrfansi/easypanel-cli/compare/v0.5.0...HEAD
[0.5.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.5.0
[0.4.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.4.0
[0.3.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.3.0
[0.2.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.2.0
[0.1.0]: https://github.com/mrfansi/easypanel-cli/releases/tag/v0.1.0
