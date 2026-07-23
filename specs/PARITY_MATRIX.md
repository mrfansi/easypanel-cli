# PARITY MATRIX — Cloudflare TUI vs EasyPanel TUI

EasyPanel = referensi kebenaran. Bukti = capture di `docs/evidence/`.
Status: ✓ selaras · ✗ beda (harus diselaraskan) · sengaja-beda (alasan wajib) · ? bukti belum lengkap.

Baseline run #1 (2026-07-23): EP di `viding-idc` (produksi, read-only), CF happy di
`pt-karya-kaya-bahagia`, error-path `mrfansi-dev` (lihat catatan butir 10). Terminal 120×35.
Evidence dir: `docs/evidence/2026-07-23-bootstrap/`.

| # | Perilaku | EasyPanel (ref) | Cloudflare (kini) | Status | Bukti | Catatan |
|---|---|---|---|---|---|---|
| 1 | Header / tab bar produk | `┌ EasyPanel — viding-idc ┐` + baris tab | `┌ Cloudflare — <akun> ┐` + `DNS │ R2`; drill-in: `Cloudflare · akun · zona — records` | ✓ | ep-home, cf-zones-happy, cf-records | Pola sama; breadcrumb drill-in CF wajar |
| 2 | Key-hints di status bar | Idle: ` Ready`; hints hanya kontekstual (filter/marks) | Hint permanen penuh: `a account · Enter records · … · Esc EasyPanel` | ✗ | ep-services, cf-zones-happy | Beda nyata. Hint permanen CF ditambah run pra-loop; putuskan: selaraskan ke EP (Ready + kontekstual) ATAU catat sengaja-beda + alasan |
| 3 | Picker akun `a` (≙ `s` server) | Popup `┌ Servers ┐`, bekerja dari semua layar | Popup `┌ Cloudflare accounts ┐`, token ter-mask, `(active)`, dari semua layar | ✓ | ep-picker, cf-picker | Selaras (v0.87.4) |
| 4 | Records drill-in via Enter | Drill-in EP (Logs/Terminal) belum di-capture | Enter zona → Records, Esc kembali ke zones | ✓ | cf-records | Perilaku hidup penuh; baseline EP drill-in menyusul bila perlu |
| 5 | Filter `/` dengan count | Judul `Services (20/108)  /web▏`; status `filter: web▏  ↑↓ select · Enter apply · Esc cancel` | Judul `Zones (4 of 9) · /ed`; status `filter: ed▏  Enter apply · Esc cancel` | ✗ | ep-filter, cf-filter | Format count beda (`20/108` vs `4 of 9`); hint `↑↓ select` hilang di CF |
| 6 | Mark `v`/`V` + `Space` bulk | Judul `· ✓ 1 marked`; status bar berubah: `1 service(s) marked — [Space] to act on them, [Esc] to clear`; menu grup multi-aksi | Judul `· 7 marked` (tanpa ✓); status bar TIDAK berubah; menu `Set proxied / Set TTL / Delete` | ✗ | ep-marks-menu, cf-marks-menu | Dua gap: ✓ di judul, pesan marks di status bar |
| 7 | `r` refresh | Status ` Refreshing...` | Reload berjalan; spinner `⠙` di status | ✓ | ep-refresh-spinner, cf-refresh-spinner | Wording transien beda tipis; perilaku sama |
| 8 | Konfirmasi aksi destruktif | belum di-capture | belum di-capture | ? | — | EP: dilarang buka dialog destruktif di produksi; capture di host aman + zzz-*. CF: dialog delete ada (kode) tapi butuh bukti layar tanpa mutasi |
| 9 | Spinner saat busy | Capture hanya menangkap teks `Refreshing...` (glyph tidak terlihat) | `⠙` terlihat | ? | ep-refresh-spinner, cf-refresh-spinner | Perlu capture EP saat spinner aktif untuk perbandingan adil |
| 10 | Empty vs loading vs error | belum lengkap | belum lengkap | ? | cf-home-noaccount | **PENTING run #1: `mrfansi-dev` kini BERHASIL membaca records (12 record)** — fixture error-path read tidak lagi mereproduksi error. Bukti error-state butuh cara lain (lihat LOOP_LEARNINGS) |
| 11 | Klik mouse | belum diuji | belum diuji | ? | — | tmux send mouse-event perlu riset; run tersendiri |
| 12 | Menu klik-kanan | belum di-capture | belum di-capture | ? | — | Regression test ada (v0.87.7, R2 Objects); bukti layar menyusul |
| 13 | Command palette `:` | `┌ Search: ▏ · actions for <svc> ┐` — daftar aksi bisa dicari | Handler ada (`keys.rs:1318`, open_cf_palette) — belum di-capture | ? | ep-palette | Capture CF palette + bandingkan wording judul |

## Ringkasan

- ✓ 4 · ✗ 3 · ? 6 (dari 13)
- Prioritas run berikutnya (skor §Fase 1): butir 6 (marks: sering dipakai, dua gap kecil, ukuran kecil) → butir 5 (filter wording) → butir 2 (butuh keputusan sengaja-beda dulu).
