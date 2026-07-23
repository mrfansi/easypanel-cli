# LOOP LEARNINGS — easypanel-cli · Cloudflare TUI Parity

Aturan di sini MENGIKAT untuk setiap run loop. Format:
`[kategori] Kalau <situasi>, maka <tindakan>. (asal: run #N / pra-loop vX.Y.Z)`

Kategori: `[paritas]` `[ddd]` `[cf-api]` `[tui]` `[proses]`.
Maks 150 baris. Hapus aturan yang sudah dijaga kompiler/test/lint.

## Aturan

- [tui] Kalau ingin menonjolkan sel di baris yang bisa ter-highlight REVERSED, maka pakai BOLD saja — tint warna fg lebar membuat bar seleksi dua-warna. (asal: pra-loop v0.87.6)
- [tui] Kalau menambah menu klik-kanan / context menu, maka dispatch WAJIB bercabang pada state layar drill-in (mis. CfScreen Buckets vs Objects) — menu induk tidak boleh muncul di layar anak. (asal: pra-loop v0.87.7)
- [paritas] Kalau memakai helper render bersama (render_confirm, render_table, dst.) dari workspace Cloudflare, maka audit dulu apakah helper itu meng-hard-code semantik EasyPanel (label, warna, wording). (asal: pra-loop, temuan render_confirm)
- [cf-api] Kalau operasi R2 objects, maka pakai REST + Bearer token yang sama dengan buckets — BUKAN kredensial S3; izin token = account-scoped "Workers R2 Storage". (asal: pra-loop v0.85.0–v0.89.0)
- [proses] Kalau layar EasyPanel yang mau di-capture butuh membuka dialog destruktif, maka JANGAN lakukan di server produksi (viding-idc) — tunda buktinya atau pakai host aman dengan target zzz-*. (asal: run #1)
- [proses] Kalau memilih akun di picker CF mengubah flag `active` di cloudflare.json, maka snapshot berkas config sebelum sesi TUI dan kembalikan byte-identik sesudahnya. (asal: run #1)
- [cf-api] Kalau butuh error-path Cloudflare, maka JANGAN andalkan `mrfansi-dev` untuk READ — sejak 2026-07-23 tokennya berhasil membaca DNS records (12 record); ia tetap fixture untuk error MUTASI saja, dan mutasi dilarang. Cari bukti error-state lewat jalur lain (mis. jaringan diputus). (asal: run #1)
- [tui] Kalau menskrip TUI via tmux, maka ingat `W` membuka MENU workspace (perlu ↓+Enter), bukan toggle langsung — dua kali `W` membuka lalu menutup menu. (asal: run #1)
- [proses] Kalau meng-capture popup/palette TUI via tmux, maka beri jeda ≥1 detik DAN verifikasi popup benar-benar ada di hasil capture sebelum lanjut — capture terlalu cepat menghasilkan bukti kosong. (asal: run #1)
- [paritas] Kalau membandingkan fitur CF dengan EP, maka bandingkan WORDING judul dan status bar terhadap capture EP, bukan hanya keberadaan fiturnya — run #1 menemukan `· ✓ N marked` vs `· N marked`, `(20/108)` vs `(4 of 9)`, dan pesan marks di status bar yang hilang. (asal: run #1)
- [proses] Kalau menyimpan capture layar dari host/akun produksi, maka `docs/evidence/` TIDAK di-commit (gitignored) — aturan owner: topologi produksi tidak dipublikasikan; bukti cukup lokal. (asal: run #1)
