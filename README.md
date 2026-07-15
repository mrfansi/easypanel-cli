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
easypanel project create <nama>
easypanel project inspect <nama>

easypanel service deploy  <project> <service> [--type app] [--force]
easypanel service restart <project> <service> [--type app]
easypanel service start   <project> <service> [--type app]
easypanel service stop    <project> <service> [--type app]
easypanel service logs    <project> <service> [--limit 100]

easypanel stats                      # CPU/mem/disk/uptime
easypanel node list                  # node swarm cluster
```

`--type` default `app`; tipe lain (mysql, postgres, redis, mongo, …) sesuai service EasyPanel.

## Menu interaktif

Jalankan tanpa argumen (atau `easypanel menu`) untuk flow bertingkat: pilih server → kategori → project → service → aksi. Aksi yang memengaruhi service nyata (deploy/restart/stop) meminta konfirmasi.

```bash
easypanel
```

## Test

```bash
cargo test
```

## API

EasyPanel memakai gaya tRPC: `POST {url}/api/rpc/{group}/{op}`, header `Authorization: Bearer <token>`, body `{"json": <input>}`, respons `{"json": <data>}`. Spesifikasi lengkap ada di `easypanel-api.json`. Command baru cukup memanggil `EasypanelClient::call(group, op, input)` — 374 endpoint tersedia.
