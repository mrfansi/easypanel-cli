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

Tak ada `server edit`: **mengedit = `add` dengan nama yang sama**, entri lama ditimpa dan status default tetap terjaga (mis. rotasi token). Server pertama otomatis jadi default. Command lain memakai server default, atau `--server <nama>` untuk menargetkan host tertentu.

Dari TUI, tekan **`s`** kapan saja: `Enter` pilih server aktif, `n` tambah, `e` edit, `x` hapus. Form edit datang dengan URL sudah terisi; **token dibiarkan kosong = tidak diubah**, jadi mengganti URL tak memaksa mengetik ulang token.

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
easypanel domain list                               # semua domain host (source -> destination)
easypanel service domains <project> <service>       # per service (dengan id)
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

# Actions (riwayat deploy/destroy/login)
easypanel action list [--limit 25] [--project P] [--service S] [--type deployment]
easypanel action kill <id>

# Monitoring & cluster
easypanel stats                      # CPU/mem/disk/load
easypanel monitor services           # CPU/memori/network per project & service
easypanel monitor storage            # pemakaian disk per service
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

- **Dashboard** — gauge CPU/Memory/Disk, sparkline CPU history (auto-refresh ~2 detik), load average, dan tabel node cluster. Server aktif saja.
- **Hosts** — **semua** server dari `servers.json` sekaligus: CPU, memori, disk, dan load per host. Tiap host diambil di thread sendiri, jadi host lambat atau mati tak menahan yang lain; host yang gagal tampil merah beserta alasannya (unreachable, token kadaluarsa) alih-alih menggagalkan seluruh tabel. Inilah satu-satunya layar yang tak bisa digantikan panel web.
- **Actions** — riwayat action (deploy/destroy/login) dengan status, target, durasi, dan umur.
- **Monitor** — lima tile metrik berhistori (CPU, Memory, Disk, Net In, Net Out) + sub-tab **Services** (CPU/memori/network per project & service) dan **Storage** (`v` untuk berganti).
- **Domains** — semua domain host: source → destination (service internal atau server custom beserta bobotnya). Form edit mencakup **SSL resolver** (`certificateResolver`) dan **wildcard**. Nama resolver ditentukan konfigurasi Traefik server (mis. `google`); server menolak nama yang tak terdaftar.
- **Projects** — panel Projects ↔ Services; buat project/service baru, atur source & build, buka view, atau jalankan aksi (deploy/restart/stop/start) dengan konfirmasi.
- **Viewer** — pane scrollable untuk logs, env, ports, mounts, domains, database backups, dan source & build.

Keybindings:

| Tombol | Aksi |
|---|---|
| `1`–`7`, `Tab` | pindah tab (`2` = Hosts, semua server sekaligus) |
| `n` · `x` | buat · hapus (Projects: project/service sesuai panel yang difokus; Domains: domain) |
| `e` · `P` | Domains: edit · jadikan primary |
| `E` | Projects: edit env service di `$EDITOR` |
| `U` · `B` | Projects: atur source · build (service app) |
| `v` | Monitor: ganti Services ↔ Storage |
| `↑↓` / `jk` | navigasi · `←→` pindah panel |
| `Enter` | buka project → services; pada service → logs |
| `e` `p` `m` `o` `b` `u` | view env · ports · mounts · domains · backups · source & build |
| `d` `R` `S` `T` | deploy · restart · stop · start (dengan konfirmasi) |
| `s` | daftar server: `Enter` pilih · `n` tambah · `e` edit (token kosong = tak diubah) · `x` hapus |
| `r` | refresh · `q` keluar |

Di dalam form: `Tab` pindah field, `Esc` batal, `Enter` simpan. Field pilihan (project, service, repo, branch, tipe, protocol) membuka dropdown yang bisa dicari dengan mengetik — bukan diketik bebas, supaya tidak ada salah ketik yang mengarahkan domain ke service yang tak ada.

Form **source** (`U`) mengikuti tipe yang dipilih: `github` memberi dropdown repo (dari `github/searchRepos`) dan branch (`github/searchBranches`, dimuat ulang tiap repo berganti), `git` memakai URL + ref bebas, `image` memakai image + kredensial registry. Form **build** (`B`) sama polanya untuk nixpacks/railpack/dockerfile/buildpacks.

Catatan penting: `updateSourceGithub` **selalu mereset `autoDeploy` jadi false** di sisi server. Karena itu form punya toggle **Auto deploy** dan CLI memasang ulang nilainya lewat `enableGithubDeploy`/`disableGithubDeploy` setelah update — tanpa ini, sekadar mengganti branch akan mematikan auto-deploy diam-diam.

Network berjalan di worker thread terpisah, jadi UI tidak pernah membeku saat request lambat. Ada dua lajur worker: aksi user dan polling metrik, supaya polling tak pernah menahan aksi user.

Metrik memakai grup **`metrics`** (Prometheus), bukan `monitorOld`: ~0,3 detik vs ~2,3 detik, dan sudah menyediakan laju network, load average, serta byte used/total. Satu panggilan `metrics/getSystemStats` memberi nilai terkini **dan** historinya, jadi sparkline datang dari server.

Catatan: sub-tab **Docker Events** milik panel tidak tersedia — itu live stream, bukan bagian dari REST API yang terdokumentasi (`easypanel-api.json`).

**Middlewares** (grup `middlewares`: 14 tipe bergaya Traefik — basicAuth, rateLimit, redirectScheme, …) belum bisa diedit dari TUI, tapi **selalu dilestarikan** saat mengedit domain, jadi tak ada yang hilang. Editornya belum dibuat karena belum ada kebutuhan nyata yang bisa diuji; kalau kamu mulai memakai middleware, ini yang berikutnya.

## Test

```bash
cargo test
```

## API

EasyPanel memakai gaya tRPC: `POST {url}/api/rpc/{group}/{op}`, header `Authorization: Bearer <token>`, body `{"json": <input>}`, respons `{"json": <data>}`. Spesifikasi lengkap ada di `easypanel-api.json`. Command baru cukup memanggil `EasypanelClient::call(group, op, input)` — 374 endpoint tersedia.
