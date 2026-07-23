# LOOP STATE — easypanel-cli · Cloudflare TUI Parity

Prompt loop: v2.0 (lihat riwayat di prompt). Cron sesi: tiap jam di :37.

## Tugas berjalan

- Run #4 (berikutnya): butir 2 — key-hints status bar: hint permanen CF vs `Ready` EP.
  Ini run KEPUTUSAN: baca komentar desain render.rs (blok render_status CF), putuskan
  sengaja-beda (tulis alasan di matriks) ATAU selaraskan ke EP. Skor 3×2÷1 = 6.
  Kalau selesai cepat, tambah bukti murah: capture palette CF (butir 13) + spinner EP (butir 9).

## Run Log

Format: `Run #N | butir paritas | iterasi sampai lulus | rework? (sebab) | gerbang gagal | rilis? (ya/tidak + alasan)`

- Run #1 (2026-07-23) | BOOTSTRAP matriks + baseline | 1 | rework ringan (capture palette terlalu cepat; `W` dikira toggle padahal menu) | 0 | tidak — doc-only, tanpa perubahan kode
- Run #2 (2026-07-23) | butir 6 marks | 2 (kritik MAJOR: marks bocor lintas tab produk) | ya — fix `cf_set_product` + regression test | 0 | ya v0.90.0 — butir 6 ✗→✓
- Run #3 (2026-07-23) | butir 5 filter | 1 (kritik SHIP tanpa major) | ringan — fixture test CF butuh `cf.accounts` terisi, bukan hanya `cf.active` | 1 (test gagal sekali: fixture) | ya v0.91.0 — butir 5 ✗→✓, sekalian padding judul

## Metrik

- Butir matriks: 13 · ✓ 6 · ✗ 1 · ? 6 (run #3).
- Baseline test: 369 lulus, 0 gagal (cargo test, run #3).

