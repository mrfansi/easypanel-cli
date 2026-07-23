# LOOP STATE — easypanel-cli · Cloudflare TUI Parity

Prompt loop: v2.0 (lihat riwayat di prompt). Cron sesi: tiap jam di :37.

## Tugas berjalan

- Run #3 (berikutnya): butir 5 — filter `/`: samakan format count judul dengan EP
  (`(20/108)`, bukan `(4 of 9)`) + hint filter `↑↓ select` di status bar CF.
  Skor: 4×2÷1 = 8. Baseline: ep-filter.txt & cf-filter.txt (bootstrap).
  Cek dulu: apakah ↑↓ memang berfungsi di filter CF (cf_filter_arrows test ada) —
  kalau ya, hint-nya saja yang kurang.

## Run Log

Format: `Run #N | butir paritas | iterasi sampai lulus | rework? (sebab) | gerbang gagal | rilis? (ya/tidak + alasan)`

- Run #1 (2026-07-23) | BOOTSTRAP matriks + baseline | 1 | rework ringan (capture palette terlalu cepat; `W` dikira toggle padahal menu) | 0 | tidak — doc-only, tanpa perubahan kode
- Run #2 (2026-07-23) | butir 6 marks | 2 (kritik MAJOR: marks bocor lintas tab produk) | ya — fix `cf_set_product` + regression test | 0 | ya v0.90.0 — butir 6 ✗→✓

## Metrik

- Butir matriks: 13 · ✓ 5 · ✗ 2 · ? 6 (run #2).
- Baseline test: 368 lulus, 0 gagal (cargo test, run #2).

