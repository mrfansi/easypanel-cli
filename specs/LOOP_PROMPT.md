# LOOP PROMPT — easypanel-cli · Cloudflare TUI Parity  ·  v2.0

> Cara pakai (sesi Claude Code baru): ketik `/loop 1h ` lalu tempel SELURUH isi
> file ini setelahnya (mulai dari "## 0. PERAN & MISI"), atau cukup minta:
> "baca specs/LOOP_PROMPT.md lalu jadwalkan sebagai loop 1 jam dan jalankan run
> pertama sekarang". Loop berjalan session-only; menutup sesi menghentikannya.

## 0. PERAN & MISI

Membuat **TUI Cloudflare berperilaku PERSIS seperti TUI EasyPanel**. EasyPanel adalah referensi kebenaran; Cloudflare menyesuaikan diri, bukan sebaliknya. Ini di atas bug-hunt umum.

Satu run menghasilkan **tiga** keluaran wajib:
1. **SATU ketidaksesuaian paritas diselaraskan** — utuh, terverifikasi di layar.
2. **Matriks paritas diperbarui** — status berubah, bukti terlampir.
3. **SATU perbaikan pada loop ini sendiri** — pelajaran terekam atau patch prompt.

Baca section *"FOCUS (owner): Cloudflare TUI — mirror EasyPanel behaviour"* di `.github/AGENT_BRIEF.md` tiap run.

---

## 1. INVARIAN (tidak bisa dinegosiasi, tidak boleh dilemahkan patch mana pun)

1. **DDD wajib untuk kode BARU maupun perubahan** — bukan hanya saat refactor. Domain murni (tipe + fungsi tanpa I/O) → application (orchestration/commands) → infrastructure (client/HTTP, worker/render/keys). Aturan domain hidup di modul domainnya (`cloudflare.rs`, `backup.rs`, `uptime.rs`), tidak tersebar di worker/render/keys.
2. **Cloudflare READ-ONLY.** Tidak ada mutasi DNS produksi, titik. Tanpa pengecualian, tanpa "cuma satu record uji".
3. **EasyPanel: hanya target buangan `zzz-*`**, dan dibersihkan setelah dipakai.
4. **Config user tidak diubah.** Kalau terpaksa, dikembalikan persis seperti semula dan disebut di laporan.
5. **Lihat layar sebelum bilang selesai.** Evidence sebelum assertion. Langkah gagal/dilewati disebut terus terang beserta outputnya.
6. **Jangan mengarang pekerjaan untuk membenarkan rilis.** Tidak ada yang layak dikirim → bilang dan berhenti. Itu run yang sah.
7. **Refactor tidak dicampur fitur** dalam satu commit. Refactor murni = tanpa bump versi.
8. **`mrfansi-dev` bukan bug.** Token tanpa izin `Zone:DNS` adalah fixture error-path permanen. Jangan pernah "memperbaikinya".

---

## 2. BERKAS STATUS (dibaca tiap run, ditulis tiap run)

| Berkas | Isi | Ditulis loop? |
|---|---|---|
| `.github/AGENT_BRIEF.md` | fokus owner | tidak (read-only) |
| `specs/PARITY_MATRIX.md` | **inti loop** — daftar perilaku, status paritas, bukti | ya |
| `specs/LOOP_STATE.md` | tugas berjalan, Run Log, metrik | ya |
| `specs/LOOP_LEARNINGS.md` | aturan hasil belajar, jebakan, kuirk API CF | ya |
| `CHANGELOG.md` | perubahan + kenapa penting | ya |
| `docs/evidence/<tgl>-<butir>/` | pasangan capture EP vs CF, diff nama test | ya |

**Bootstrap:** kalau `PARITY_MATRIX.md` belum ada, **membuatnya adalah tugas run pertama** (isi dari §5, plus capture baseline tiap layar EasyPanel yang setara). Run berikutnya baru menyentuh kode.

---

## 3. PROTOKOL RUN (urut, jangan lompat)

### Fase 0 — Orientasi (≤10% anggaran run)
`LOOP_LEARNINGS.md` (aturan tercatat bersifat **mengikat**) → `PARITY_MATRIX.md` → `AGENT_BRIEF.md` → `LOOP_STATE.md`.
Pakai **graphify untuk orientasi struktur sebelum grep**, bukan sesudah.
Keluaran: 3 baris — status paritas keseluruhan (x/y ✓), butir ✗ paling menyakitkan, kandidat tugas.

### Fase 1 — Pilih SATU butir paritas (deterministik)
```
Skor = (Seberapa sering pengguna menabraknya 1–5  ×  Seberapa jauh dari EasyPanel 1–5)  ÷  Ukuran 1–5
```
Ambil skor tertinggi yang pasti selesai dalam satu run. Ragu → ambil irisan lebih kecil yang tetap utuh (satu perilaku hidup penuh, bukan setengah).

**Definisi SELARAS** (semua harus "ya"):
- [ ] Keybinding, penempatan, dan wording sama dengan EasyPanel
- [ ] Alur tanpa dead-end; setiap aksi punya jalan mundur
- [ ] State kosong / loading / error dibedakan jelas — ketiganya dites
- [ ] Kedua akun CF dijalankan: `pt-karya-kaya-bahagia` (happy path) **dan** `mrfansi-dev` (error path)
- [ ] Perbedaan yang tersisa dari EasyPanel hanya yang **sengaja**, dan alasannya ditulis di matriks

### Fase 2 — Tangkap baseline EasyPanel dulu
Sebelum menyentuh kode Cloudflare, jalankan layar EasyPanel yang setara dan capture. Ini kontraknya. **Dilarang menyelaraskan berdasarkan ingatan.**
Simpan pasangan ke `docs/evidence/<tgl>-<butir>/`: `ep-before.txt`, `cf-before.txt`.

### Fase 3 — Implement (DDD, lihat §6)
Keputusan/aturan → layer domain. Worker/render tinggal pemanggil + presentasi.
Pakai **ponytail untuk menahan over-engineering**, **TDD** untuk logika domain, **agent paralel** untuk audit/fan-out, **obsidian** untuk mencatat keputusan yang punya konsekuensi jangka panjang.

### Fase 4 — Verifikasi di layar (WAJIB)
Jalankan biner. Capture `cf-after.txt`. Sandingkan dengan `ep-before.txt` dan sebutkan perbedaan yang tersisa satu per satu — nol perbedaan tak terjelaskan, atau butir belum selesai.
Uji juga di terminal sempit (80×24) supaya tidak ada yang terpotong.

### Fase 5 — Kritik (agent critic/designer, jujur)
Layout: ruang, keterbacaan, tak terpotong. Behaviour: tanpa dead-end, umpan balik jelas, tak ada aksi diam-diam. Design: warna bermakna, konsisten, kontras cukup.
Maksimal **3 iterasi** perbaikan. Belum beres juga → descope butirnya, catat sebabnya sebagai jebakan di `LOOP_LEARNINGS.md`.

### Fase 6 — Gerbang mutu
- `cargo fmt` (jalankan, bukan cuma `--check`) · `cargo clippy --all-targets -- -D warnings` · `cargo test` — bersih.
- **Kalau run ini refactor:** simpan `cargo test -- --list` sebelum & sesudah ke evidence, dan tunjukkan diff-nya **kosong**. Klaim "behaviour-preserving" tanpa diff ini tidak sah.

### Fase 7 — Rilis (dengan gerbang anti-mengarang)
Perbarui `PARITY_MATRIX.md` + `CHANGELOG.md`. Rilis **hanya bila** ada perubahan user-visible nyata: status matriks berubah ✗ → ✓, atau bug benar-benar diperbaiki. Kalau tidak → jangan bump, jangan tag, katakan apa adanya.
Release notes menjelaskan **apa yang berubah dan kenapa itu penting bagi pengguna**, bukan meringkas diff.

### Fase 8 — SELF-IMPROVE (WAJIB — run tanpa fase ini dianggap gagal)
Lihat §4.

### Fase 9 — Laporan
Template §9, persis.

---

## 4. PROTOKOL SELF-IMPROVING

Siklus tiap run: **Terapkan → Rekam → Rawat**.

### A. Terapkan (sebelum coding)
Sebutkan aturan `LOOP_LEARNINGS.md` mana yang relevan dan bagaimana dipatuhi. Melanggar aturan tercatat tanpa alasan tertulis = regresi.

### B. Rekam (sesudah gerbang mutu)
Tambah entri Run Log ke `LOOP_STATE.md`:
```
Run #N | butir paritas | iterasi sampai lulus | rework? (sebab) | gerbang gagal berapa kali | rilis? (ya/tidak + alasan)
```
Lalu tambah **minimal 1 aturan baru**, atau nyatakan eksplisit "tidak ada pelajaran baru" + alasan.

Format (satu baris, bisa diuji):
```
[kategori] Kalau <situasi konkret>, maka <tindakan konkret>.  (asal: run #N)
```
Kategori yang berlaku: `[paritas]` `[ddd]` `[cf-api]` `[tui]` `[proses]`.

**Generalisasi paritas — ini yang membuat loop makin cepat:** setiap ketidaksesuaian yang ditemukan wajib ditanya *"perilaku sekelas ini muncul di layar mana lagi?"*. Jawabannya jadi aturan, dan butir-butir baru langsung ditambahkan ke matriks sebagai ✗ — jangan tunggu ditemukan ulang lima run kemudian.
> contoh: `[paritas] Kalau layar menampilkan daftar, maka wajib ada filter / dengan count, r refresh, dan mark v/V + Space — cek ketiganya sekaligus. (asal: run #6)`

**Sumber aturan yang sah** hanya kejadian nyata run ini: rework, gagal clippy/test, kritik yang mengulang, gesekan saat memakai biner, kuirk API Cloudflare, salah menaruh layer DDD. Hipotesis dan "sebaiknya nanti…" **bukan** aturan.

### C. Rawat
Gabungkan duplikat. **Hapus aturan yang sudah dijaga kompiler, tipe, test, atau lint.** `LOOP_LEARNINGS.md` maksimal **150 baris**; lewat batas → pangkas yang paling jarang terpakai.

### D. Meta-review (tiap 5 run)
Baca tren Run Log: iterasi turun? rework menumpuk di area yang sama? butir matriks tuntas per run naik?
Hasilkan patch untuk prompt ini:
```
PATCH v<lama> → v<baru>
Bagian : §x.y
Lama   : "<teks lama>"
Baru   : "<teks baru>"
Alasan : run #A dan #C gagal karena ...
Metrik : diharapkan <metrik> membaik dari X ke Y
```
**Guardrail (mutlak):** (i) invarian §1 tak boleh dihapus/dilemahkan; (ii) maks 2 bagian per patch; (iii) penambahan >5 baris harus disertai penghapusan setara; (iv) hanya sah kalau menunjuk kegagalan nyata di Run Log, bukan hipotesis; (v) naikkan versi & catat di riwayat.

### E. Protokol kegagalan
- Butir sama gagal 2 run beruntun → wajib dipecah + tulis aturan penyebabnya.
- Kritik gagal 3 iterasi → descope.
- Salah menaruh layer DDD → perbaiki penempatannya dulu, baru lanjut fitur.

### F. Lintas-repo
Aturan berkategori `[tui]`, `[ddd]`, dan `[proses]` bersifat portabel — salin ke `LOOP_LEARNINGS.md` repo lain yang memakai loop serupa (mis. akunting-cli). Aturan `[paritas]` dan `[cf-api]` tetap lokal.

---

## 5. MATRIKS PARITAS — butir awal

Format tiap baris: `| perilaku | EasyPanel (ref) | Cloudflare (kini) | status ✓/✗/sengaja-beda | bukti | catatan |`

1. Header / tab bar produk
2. Key-hints di status bar
3. Picker akun lewat `a`, **bukan** tab
4. Records drill-in
5. Filter `/` dengan count
6. Mark `v` / `V` + `Space` untuk bulk
7. `r` refresh
8. Konfirmasi aksi destruktif
9. Spinner saat busy
10. Bedanya empty vs loading vs error
11. Klik mouse
12. Menu klik-kanan
13. Command palette `:`

Butir baru dari generalisasi (§4.B) ditambahkan ke daftar ini, tidak disimpan di kepala.

---

## 6. CHECKLIST DDD (gerbang, dicek tiap run sebelum commit)

- [ ] Aturan/keputusan baru ada di modul domain, bisa dites **tanpa** I/O
- [ ] `worker.rs` / `render.rs` / `keys.rs` tidak mengandung percabangan aturan domain — hanya memanggil & menyajikan
- [ ] Bounded context jelas (services · domains · backups · monitoring · cloudflare) dan tidak bocor lintas konteks
- [ ] Tipe domain tidak membawa tipe HTTP/klien
- [ ] Ada test unit untuk fungsi domain baru (TDD kalau logikanya nontrivial)

Kalau satu pun tidak tercentang, kode belum siap commit.

---

## 7. PROTOKOL AKUN LIVE

**Preflight** (sebelum panggil API):
- [ ] Sebutkan akun mana yang dipakai dan untuk apa
- [ ] Cloudflare: konfirmasi operasi read-only — daftar endpoint yang akan dipanggil, semuanya `GET`
- [ ] EasyPanel: target bernama `zzz-*` saja

**Postflight:**
- [ ] Resource `zzz-*` dibersihkan
- [ ] Config user utuh (atau dikembalikan + disebut di laporan)
- [ ] Nol mutasi DNS — nyatakan eksplisit di laporan

`mrfansi-dev` = error-path (tanpa `Zone:DNS`). `pt-karya-kaya-bahagia` = happy-path live, read-only. **Keduanya dijalankan tiap kali menyentuh layar Cloudflare.**

---

## 8. DEFINITION OF DONE

- [ ] Satu butir paritas selaras & terlihat di layar
- [ ] Pasangan capture EP↔CF tersimpan, perbedaan tersisa nol atau terjelaskan
- [ ] Kedua akun CF diuji (happy + error path)
- [ ] Checklist DDD §6 penuh
- [ ] fmt (dijalankan) · clippy `-D warnings` · test — bersih; refactor: diff nama test kosong
- [ ] `PARITY_MATRIX.md` + `CHANGELOG.md` diperbarui
- [ ] Rilis hanya kalau layak; kalau tidak, dikatakan
- [ ] `LOOP_STATE.md` + `LOOP_LEARNINGS.md` diperbarui
- [ ] Laporan §9 lengkap

---

## 9. TEMPLATE LAPORAN AKHIR RUN

```
RUN #N — butir paritas: <nama>

1. SELARAS     : <perilaku yang kini sama dengan EasyPanel>
   BUKTI       : docs/evidence/<tgl>-<butir>/ (ep-before, cf-before, cf-after)
   SISA BEDA   : <nol / daftar + alasan sengaja>
2. MATRIKS     : <x>/<y> ✓ (sebelumnya <x-1>/<y>); butir baru ditambahkan: <n>
3. DDD         : checklist §6 penuh — domain <modul> menerima <aturan apa>
4. AKUN        : happy ✓ / error ✓ · nol mutasi DNS · zzz-* bersih
5. GERBANG     : fmt ✓ · clippy ✓ · test ✓ (<jumlah>) · [refactor: diff nama test kosong ✓]
6. RILIS       : ya vX.Y.Z / tidak — <alasan>
7. PELAJARAN   : [kategori] Kalau ..., maka ...   (atau: tidak ada + alasan)
8. PATCH PROMPT: ada / tidak — <bagian & alasan>
9. BERIKUTNYA  : <butir berikutnya> — skor <n>, kenapa prioritas, perkiraan ukuran
10. TERUS TERANG: <langkah gagal/dilewati + outputnya, atau "tidak ada">
```

---

## RIWAYAT PROMPT

- **v2.0** — restrukturisasi dari v1: matriks paritas sebagai inti loop (ganti kritik ad-hoc tiap run), baseline EasyPanel wajib di-capture sebelum menyelaraskan, aturan pemilihan butir deterministik, checklist DDD sebagai gerbang commit, protokol preflight/postflight akun live, diff nama test sebagai bukti refactor, protokol self-improving (Terapkan/Rekam/Rawat/Meta-review) + generalisasi paritas + guardrail patch.
