# easypanel

CLI untuk mengelola **banyak host EasyPanel** — projects, services, monitoring/logs, dan node cluster. Ditulis dalam Rust.

## Build

```bash
cargo build --release
# binary: target/release/easypanel
```

## Konfigurasi server

Kredensial disimpan di `~/.config/easypanel/servers.json` (perms `0600`), dikelola lewat command:

```bash
easypanel server add prod --url https://panel.example.com --token <TOKEN>
easypanel server add prod            # interaktif (prompt url + token)
easypanel server list
easypanel server use prod            # jadikan default
easypanel server remove prod
```

Server pertama otomatis jadi default. Command lain memakai server default, atau `--server <nama>` untuk menargetkan host tertentu.

## Command

```bash
easypanel project list
easypanel project create  <nama>
easypanel project inspect <nama>
easypanel project destroy <nama> [--yes]

# Deploy & lifecycle
easypanel service create  <project> <service> [--type app]
easypanel service deploy  <project> <service> [--type app] [--force]
easypanel service restart <project> <service> [--type app]
easypanel service start   <project> <service> [--type app]
easypanel service stop    <project> <service> [--type app]
easypanel service destroy <project> <service> [--type app] [--yes]
easypanel service logs    <project> <service> [--limit 100]

# Environment (set-env menimpa seluruh env, baca dari --file atau stdin)
easypanel service env     <project> <service> [--type app]
easypanel service set-env <project> <service> [--type app] [--file .env]

# Ports
easypanel service ports       <project> <service>
easypanel service port-add    <project> <service> --published <n> --target <n> [--protocol tcp]
easypanel service port-remove <project> <service> --index <n>

# Mounts
easypanel service mounts       <project> <service>
easypanel service mount-add    <project> <service> --kind volume --name <vol> --mount-path /data
easypanel service mount-add    <project> <service> --kind bind --host-path /srv/x --mount-path /data
easypanel service mount-remove <project> <service> --index <n>

# Domains
easypanel service domains <project> <service>       # list (dengan id)
easypanel domain delete       <id>
easypanel domain set-primary  <id>

# Databases & backups
easypanel service databases      <project> <service>   # db dalam service database
easypanel service backups        <project> <service>   # jadwal backup database
easypanel service volume-backups <project> <service>   # jadwal backup volume
easypanel backup db-run       <id>    # jalankan backup db sekarang
easypanel backup db-delete    <id>
easypanel backup volume-run   <id>
easypanel backup volume-delete <id>

# Certificates & notifications
easypanel certificate list
easypanel certificate remove <domain>
easypanel notification list
easypanel notification delete <id>

# Monitoring & cluster
easypanel stats                      # CPU/mem/disk/uptime
easypanel node list                  # node swarm cluster
```

## Install

```bash
./install.sh                 # build release + pasang ke /usr/local/bin
PREFIX=~/.local/bin ./install.sh
```

Rilis biner (macOS/Linux) dibuat otomatis oleh GitHub Actions saat push tag `v*`.

`--type` default `app`; tipe lain (mysql, postgres, redis, mongo, mariadb, wordpress, compose, …) sesuai service EasyPanel. Ports, mounts, dan domains dipanggil per project+service (tanpa `--type`).

## TUI (dashboard interaktif)

Jalankan tanpa argumen (atau `easypanel menu`) untuk membuka TUI full-screen (ratatui):

```bash
easypanel                 # server default
easypanel --server prod   # host tertentu
```

- **Dashboard** — gauge CPU/Memory/Disk, sparkline CPU history (auto-refresh ~2 detik), uptime, dan tabel node cluster.
- **Projects** — panel Projects ↔ Services; buat project/service baru, buka view, atau jalankan aksi (deploy/restart/stop/start) dengan konfirmasi.
- **Viewer** — pane scrollable untuk logs, env, ports, mounts, domains, dan database backups.

Keybindings:

| Tombol | Aksi |
|---|---|
| `1/2/3`, `Tab` | pindah tab |
| `↑↓` / `jk` | navigasi · `←→` pindah panel |
| `Enter` | buka project → services; pada service → logs |
| `e` `p` `m` `o` `b` | view env · ports · mounts · domains · backups |
| `d` `R` `S` `T` | deploy · restart · stop · start (dengan konfirmasi) |
| `s` | ganti server (bila ada >1 host) |
| `r` | refresh · `q` keluar |

Network berjalan di worker thread terpisah, jadi UI tidak pernah membeku saat request lambat.

## Test

```bash
cargo test
```

## API

EasyPanel memakai gaya tRPC: `POST {url}/api/rpc/{group}/{op}`, header `Authorization: Bearer <token>`, body `{"json": <input>}`, respons `{"json": <data>}`. Spesifikasi lengkap ada di `easypanel-api.json`. Command baru cukup memanggil `EasypanelClient::call(group, op, input)` — 374 endpoint tersedia.
