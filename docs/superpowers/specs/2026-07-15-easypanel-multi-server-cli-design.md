# EasyPanel Multi-Server CLI — Design

Date: 2026-07-15
Status: Approved

## Tujuan

CLI (Laravel Zero, binary `easypanel`) untuk mengelola **banyak host EasyPanel**
dari satu tempat: tiap host punya URL + token sendiri, dan tiap host bisa punya
banyak node. Fokus v1: **Projects & Services** + **Monitoring & Logs**.

## Fakta API (terverifikasi terhadap host asli)

- Gaya tRPC di path `POST {url}/api/rpc/{group}/{op}`.
- Auth: header `Authorization: Bearer <token>`.
- Body **selalu wajib** `Content-Type: application/json`; query tanpa parameter
  tetap harus mengirim `{"json": null}` (body kosong → 400 `FST_ERR_CTP_EMPTY_JSON_BODY`).
- Input berparameter: `{"json": {<params>}}`.
- Response envelope: `{"json": <data>, "meta": [...]}` → unwrap `.json` di top-level.
  (Bukan `result.data.json` seperti tRPC default.)

## Arsitektur — 3 lapis

### 1. Config store
- Lokasi: `~/.config/easypanel/servers.json`, permission `0600`.
- Isi: array `{ "name": string, "url": string, "token": string, "default": bool }`.
- Dikelola **hanya** lewat command (`server:add/list/use/remove`), tidak edit manual.
- Satu server ditandai `default: true`. Command lain memakai server default kecuali
  diberi `--server=<name>`.
- Service class `ServerConfig`: `all()`, `get(name)`, `default()`, `add(...)`,
  `remove(name)`, `setDefault(name)`. Bertanggung jawab baca/tulis file + chmod.

### 2. EasypanelClient
- Dibuat dari satu server config (`url`, `token`).
- Method inti: `call(string $group, string $op, mixed $input = null): mixed`
  - `POST {url}/api/rpc/{group}/{op}` dengan header Bearer + body `{"json": $input}`.
  - Sukses → return `$response['json']`.
  - Gagal (non-2xx) → lempar `EasypanelException` dengan pesan dari body EasyPanel
    (mis. `message`) + status code. 401 → pesan "token invalid/expired".
- Pakai Laravel `Http` facade (Guzzle) supaya mudah di-fake untuk test.

### 3. Commands
- `BaseServerCommand` (abstract): resolve server aktif dari `--server=` atau default,
  bangun `EasypanelClient`, sediakan helper `$this->client`. Jika tidak ada server
  terkonfigurasi → pesan error jelas + saran `server:add`.
- Command konkret memakai `$this->client->call(...)` dan memformat output (tabel/JSON).

## Daftar Command v1

### Server (lokal, tanpa API)
| Command | Aksi |
|---|---|
| `server:add` | Tanya/terima `name`, `url`, `token`; simpan. Server pertama jadi default. |
| `server:list` | Tabel semua server (tandai default). Token disamarkan. |
| `server:use <name>` | Set default. |
| `server:remove <name>` | Hapus server. |

`server:add` menerima argumen/opsi non-interaktif (`--url`, `--token`) atau prompt
interaktif bila kosong.

### Projects
| Command | Endpoint | Input |
|---|---|---|
| `project:list` | `projects/listProjects` | `null` |
| `project:create <name>` | `projects/createProject` | `{name}` (validasi `^[a-z0-9-_]+$`) |
| `project:inspect <name>` | `projects/inspectProject` | `{projectName}` → tampilkan services + status |

### Services (default `--type=app`)
| Command | Endpoint | Input |
|---|---|---|
| `service:deploy <project> <service> [--force]` | `services/{type}/deployService` | `{projectName, serviceName, forceRebuild}` |
| `service:restart <project> <service>` | `services/{type}/restartService` | `{projectName, serviceName}` |
| `service:start <project> <service>` | `services/{type}/startService` | `{projectName, serviceName}` |
| `service:stop <project> <service>` | `services/{type}/stopService` | `{projectName, serviceName}` |

`--type` mendukung tipe service EasyPanel (app, mysql, postgres, mongo, redis,
mariadb, dst). Endpoint deploy/restart/start/stop ada per tipe.

### Monitoring & Logs
| Command | Endpoint | Input |
|---|---|---|
| `stats` | `monitorOld/getSystemStats` | `null` → tampilkan CPU/mem/disk/uptime |
| `service:logs <project> <service> [--limit=100]` | `logs/queryServiceLogs` | `{projectName, serviceName, limit}` |

### Cluster
| Command | Endpoint | Input |
|---|---|---|
| `node:list` | `cluster/listNodes` | `null` |

## Error Handling
- `EasypanelClient` melempar `EasypanelException` untuk semua kegagalan HTTP/API.
- `BaseServerCommand` menangkap `EasypanelException`, cetak pesan ringkas ke stderr,
  return exit code non-zero (1). Tidak ada stack trace ke user.
- Config store: file corrupt / tidak ada → treat sebagai kosong; command server:*
  tetap jalan, command API memberi pesan "belum ada server, jalankan server:add".

## Testing (minimal, Pest)
- `EasypanelClient`: `Http::fake` — verifikasi URL, header Bearer, body `{"json":...}`,
  unwrap `.json`, dan pelemparan exception saat non-2xx.
- `ServerConfig`: add → default otomatis untuk server pertama, setDefault memindah
  flag, remove, permission `0600`. Pakai direktori temp (tidak sentuh `~/.config`).
- Tidak ada test yang memanggil host asli.

## Di luar scope v1 (YAGNI — tambah saat butuh)
191 endpoint services lengkap (update env/build/resources/domains/ports/mounts),
databaseBackups, volumeBackups, storageProviders, branding, certificates,
cloudflareTunnel, middlewares, twoFactor, users, settings. Client `call()` generik
sudah cukup untuk menambah command tipis kapan pun tanpa mengubah arsitektur.

## Catatan
- Repo belum di-`git init`. Spec ini ditulis ke file; commit dilakukan setelah repo
  diinisialisasi (ditawarkan saat implementasi).
