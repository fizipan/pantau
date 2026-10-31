<p align="center">
  <img src="docs/logo/icon-256.png" alt="Logo Pantau: meteran bulat dengan busur biru dan zona kuning di atas kotak gelap berujung bulat" width="128" height="128">
</p>

<h1 align="center">Pantau</h1>

<p align="center">
  Lihat apa yang memakai Mac kamu, lalu hentikan atau bersihkan dengan satu klik. Aplikasi menu bar macOS untuk CPU, memori, dan disk.
</p>

<p align="center">
  <a href="https://pantau-app-swart.vercel.app"><img alt="Situs" src="https://img.shields.io/badge/situs-pantau--app--swart.vercel.app-black"></a>
  <a href="https://github.com/fizipan/pantau/releases/latest"><img alt="Rilis terbaru" src="https://img.shields.io/github/v/release/fizipan/pantau"></a>
  <a href="LICENSE"><img alt="MIT License" src="https://img.shields.io/badge/license-MIT-blue"></a>
  <img alt="macOS, Apple Silicon" src="https://img.shields.io/badge/macOS-Apple%20Silicon-lightgrey">
</p>

<p align="center"><a href="README.md">English</a> · Bahasa Indonesia</p>

<p align="center">
  <img src="docs/screenshots/proses-dark.png" alt="Tab Proses mode gelap: meteran CPU, RAM, dan disk, lalu proses yang dikelompokkan per aplikasi dan tool" width="380">
  <img src="docs/screenshots/penyimpanan-light.png" alt="Tab Penyimpanan mode terang: cache yang aman dibersihkan dan item yang perlu dicek dulu" width="380">
</p>

<p align="center">
  <img src="docs/screenshots/proses-konfirmasi.png" alt="Tab Proses menanyakan Yakin? sebelum menghentikan kelompok yang memuat dua sesi AI" width="380">
</p>

<p align="center"><sub>Semua screenshot memakai data contoh.</sub></p>

```bash
brew install --cask fizipan/tap/pantau
```

Laptop dengan RAM 8 GB yang menjalankan agent AI, dev server, dan tes sekaligus gampang jadi lambat, dan sulit ditebak penyebabnya. Pantau duduk di menu bar, menyebutkan apa yang memakai CPU, memori, dan disk, lalu membiarkan kamu menghentikan proses atau membersihkan cache dari panel yang sama. Untuk satu orang di satu laptop. Tanpa helper berhak istimewa, tanpa koneksi jaringan, dan tanpa akun.

**Tampilannya baru dalam bahasa Indonesia.** Terjemahan Inggris belum ada.

## Rilis awal

Baca ini sebelum mengklik apa pun. Sebagian besar yang sudah diuji, diuji di tingkat kode. Bagian berikut **belum** diperiksa sepenuhnya di aplikasi sungguhan:

- Kill, termasuk konfirmasi untuk sesi AI
- Klik pada banner notifikasi
- Jendela Pengaturan dan mulai saat login
- Pembersihan pnpm, file sementara, dan versi lama Claude Code pada data sungguhan

Pantau bisa menghentikan proses dan menghapus folder cache. Baca konfirmasinya sebelum mengklik, dan jangan jalankan saat ada pekerjaan yang belum disimpan di proses yang tercantum. Pembersihan cache `.next` dan pembukaan panel sudah dicoba sungguhan.

## Fungsi

**Tab Proses**
- Proses digabung per aplikasi (bundle `.app`) atau per tool developer dan project: `claude`, `vitest`, `jest`, `playwright`, `next`, `vite`, `tsx`, `bun`, `codex`, `cursor-agent`. Worker `node` di bawah Vitest ikut baris Vitest. Proses lain digabung per nama dan project, jadi `node` di dua project menjadi dua baris. Nama project diambil dari folder terdekat yang berisi `.git` atau `package.json`.
- RAM memakai `phys_footprint`, angka yang sama dengan Activity Monitor. CPU berskala 0 sampai 100%.
- Port TCP yang didengarkan tampil di baris proses, dan bisa dicari (`:3000`).
- Kill: satu proses satu klik. Kelompok, sesi AI, dan target yang di dalamnya ada sesi AI butuh klik kedua (`Yakin?`), dengan peringatan yang menyebut sesi mana yang ikut mati. Sinyalnya `SIGTERM`; bila proses masih hidup setelah 5 detik muncul tombol **Paksa** (`SIGKILL`).
- Proses sisa (induknya sudah mati, tool test atau dev, tidak ada port): daftar kandidat dengan perintah, project, dan umur. Kamu yang memilih dan mengonfirmasi.
- Banner muncul saat tekanan memori tidak normal, lengkap dengan swap.

**Tab Penyimpanan**
- Bisa dibersihkan langsung: cache `.next` per project, cache pnpm (`pnpm store prune`), file sementara yang sudah lebih dari 3 hari dan tidak dibuka proses mana pun, versi lama Claude Code (bukan yang aktif, bukan yang terbaru, dan tidak sedang dijalankan sesi mana pun). Versi yang masih dijalankan sesi dilewati dan dihitung; bila daftar file terbuka tidak bisa dibaca, tidak ada versi yang dihapus.
- Hanya tombol Lihat (buka Finder): database Cursor, VM Claude Desktop, riwayat sesi Claude Code, `node_modules`, sisa instalasi Docker. Pantau tidak pernah menghapus isi `~/.claude/projects` atau `state.vscdb`.
- Daftar file 100 MB ke atas yang berubah dalam 7 hari terakhir.
- Setelah pembersihan, yang ditampilkan adalah ruang disk yang benar-benar kembali, dan hanya item itu yang diukur ulang.
- Pengukuran dan pemindaian otomatis hanya berjalan saat panel terbuka, dan berhenti saat panel ditutup. Yang sudah terukur tetap tersimpan, dan pembukaan berikutnya mulai dari yang paling lama menunggu; item yang penelusurannya terpotong (misalnya store pnpm yang besar) dikerjakan paling akhir, supaya satu folder besar tidak menahan yang lain. Pemindaian "Baru masuk" otomatis hanya tersimpan bila selesai sebelum panel ditutup. Menekan "Pindai ulang" atau "Ukur ulang" saat proses otomatis sedang jalan menjadikannya milikmu: ia jalan sampai selesai walau panel ditutup. Pemindaian pertama bisa makan beberapa menit, jadi daftar "Baru masuk" terisi bila pemindaian itu dibiarkan selesai.

**Lainnya:** grafik 60 detik, notifikasi (memori tertekan lebih dari 30 detik, disk di bawah 20 GB, jeda 15 menit per jenis), shortcut global (bawaan Ctrl+Option+M), jendela Pengaturan, tema terang dan gelap mengikuti macOS.

## Instalasi

```bash
brew install --cask fizipan/tap/pantau
```

Yang dibutuhkan: [Homebrew](https://brew.sh) dan Mac **Apple Silicon**. Cask-nya memakai `depends_on arch: :arm64`. Perbarui dengan `brew upgrade --cask pantau`, dan hapus dengan `brew uninstall --cask pantau`. Tambahkan `--zap` kalau data lokal aplikasinya juga mau dihapus.

**Aplikasi ini belum dinotarisasi Apple.** Tanda tangannya ad hoc, jadi cask menghapus tanda karantina unduhan supaya macOS mau membukanya. Kalau kamu tidak mau itu, build dari source saja. macOS bisa meminta izin folder (Documents, Downloads) lagi setelah upgrade, karena tanda tangan ad hoc berubah di tiap build.

DMG juga tersedia di [Releases](https://github.com/fizipan/pantau/releases). macOS memblokir saat pertama dibuka, jadi buka System Settings, lalu Privacy & Security, dan pilih **Open Anyway**.

Pasang dulu ke `/Applications` lalu nyalakan mulai saat login dari sana: item login menunjuk ke lokasi app saat opsi itu dinyalakan. Bila app dipindah sesudahnya, matikan dan nyalakan lagi.

### Izin macOS

- **Notifikasi:** macOS menanyakannya saat notifikasi pertama dikirim.
- **Dokumen dan Unduhan:** membaca `~/Documents` bisa memunculkan dialog izin. Bila ditolak, baris `.next` dan `node_modules` di folder itu tidak bisa ditemukan, jadi tab Penyimpanan menulis "Butuh izin untuk membaca Documents" di atas daftar. Keterangan ini tetap ada setelah restart sampai pengukuran berikutnya berhasil membaca folder itu. Tombolnya membuka Files and Folders di Pengaturan Sistem.
- **Mulai saat login:** dipasang sebagai LaunchAgent (`~/Library/LaunchAgents/Pantau.plist`) dari jendela Pengaturan.

## Privasi

Pantau **tidak membuka koneksi jaringan** dan **tidak punya telemetri atau analitik**. Content security policy webview-nya tidak mengizinkannya. Tidak ada yang keluar dari Mac kamu.

- Pengaturan dan cache pengukurannya ada di `~/Library/Application Support/local.toolslab.pantau`.
- Daftar proses, port, dan file terbuka dibaca dengan alat bawaan sistem, atas nama kamu, tanpa hak istimewa.
- Ia hanya membersihkan lokasi yang terdaftar, dan hanya setelah klik. Riwayat chat dan `state.vscdb` tidak pernah dihapus.

## Batasan

- Proses milik root atau pengguna lain tampil tanpa angka dan tanpa tombol Kill. Pantau tidak memasang helper berhak istimewa, jadi macOS tidak memberi datanya. Proses ini hanya muncul lewat pencarian.
- Pemilik file di daftar "Baru masuk atau berubah" hanya tebakan dari folder. macOS umumnya tidak mencatat aplikasi mana yang membuat file, dan Pantau tidak menulis "diunduh oleh AI".
- Pembersihan file sementara sengaja sempit dan biasanya hanya membebaskan sedikit ruang. `lsof` tanpa hak root hanya melihat file yang dibuka proses milik sendiri, jadi "tidak dibuka proses mana pun" hanya berlaku untuk proses yang terlihat.
- Ukuran di daftar adalah ukuran folder. Hasil pembersihan pnpm, file sementara, dan versi Claude bisa lebih kecil, dan layar menyebutnya "hingga".
- Banner notifikasi ditarik dari Notification Center setelah 5 menit. Kamu yang baru kembali setelah itu tidak menemukannya. Selama menunggu, CPU Pantau naik ke sekitar 1%; rata-rata 15 menit yang terukur 0,78%, dengan layar tidak terkunci.
- Setelah panel ditutup, proses WebContent hilang tetapi helper WebKit GPU dan Networking tidak langsung. Pada build `measure` total Pantau ditambah helper-nya 45 sampai 53 MB tepat setelah tutup; helper GPU keluar sekitar 75 detik kemudian, tersisa Networking (6,6 MB) dengan total 33,6 MB. Angka ini dari satu run di satu mesin, bukan dari build bawaan.
- Belum diukur apakah App Nap memperlebar jeda sampler saat layar terkunci. Bila jeda melewati 10 detik, notifikasi tekanan memori tidak terkirim selama itu. Dengan layar terbuka, jeda terbesar 2,1 detik.
- Posisi panel dihitung dalam point per layar, supaya MacBook 2x dengan monitor luar 1x tidak tertukar. Itu hanya diuji dengan angka di tes; mesin pengembangnya punya satu layar.
- Daftar proses tampil segera setelah panel terbuka, dengan CPU berupa "…" sampai bacaan kedua (sekitar setengah detik kemudian). RAM sudah benar sejak bacaan pertama.
- Hanya diuji di satu mesin (MacBook, RAM 8 GB, macOS 27.0.1). Notifikasi memakai API `NSUserNotification` yang sudah usang tetapi masih jalan.
- Tampilannya baru dalam bahasa Indonesia.
- **Mac Intel** tidak didukung oleh build rilis. Kamu bisa build dari source, tetapi itu belum pernah diuji di Intel.

## Build dari source

Yang dibutuhkan: macOS, Node.js, [pnpm](https://pnpm.io), [Rust](https://rustup.rs), dan Xcode Command Line Tools. Tanda tangannya ad hoc, jadi tidak perlu sertifikat.

```bash
git clone https://github.com/fizipan/pantau.git
cd pantau
pnpm install
pnpm tauri build --bundles app
open src-tauri/target/release/bundle/macos/Pantau.app
```

Salin ke `/Applications` untuk dipakai terus.

## Pengembangan

```bash
pnpm tauri dev                     # jalankan aplikasi
pnpm vitest run                    # tes frontend
pnpm tsc --noEmit                  # cek tipe
cd src-tauri && cargo test         # tes Rust
cd src-tauri && cargo clippy --all-targets -- -D warnings
```

Tiga fitur cargo, semuanya mati secara bawaan:
- `measure`: membuka dan menutup panel sesuai jadwal untuk pengukuran memori (`scripts/measure-webview.sh <pid>`).
- `notify-test`: mengirim satu notifikasi disk rendah 5 detik setelah start, untuk mencoba banner dan kliknya.
- `tick-log`: menulis jeda terbesar antar-sampel sistem sekali semenit, untuk memeriksa batas 10 detik pada notifikasi tekanan memori.

Dibangun dengan Tauri 2, Rust, React, TypeScript, dan Tailwind CSS. Situsnya ada di `site/`, dibuat dengan Astro, GSAP, dan Lenis. Jalankan dengan `cd site && pnpm install && pnpm dev`. Catatan desainnya ada di [docs/design/DESIGN.md](docs/design/DESIGN.md). Baca [CONTRIBUTING.md](CONTRIBUTING.md) sebelum membuka pull request, dan [SECURITY.md](SECURITY.md) untuk melaporkan celah keamanan.

## Lisensi

[MIT](LICENSE).
