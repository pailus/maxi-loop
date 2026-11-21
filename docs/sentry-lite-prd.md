# PRD: Sentry-Lite (nama sementara)

Status: draft awal (2026-11-21)
Dokumen pendamping: `sentry-lite-design.md` (desain teknis)

## 1. Ringkasan

Sentry-Lite adalah platform error tracking dan performance monitoring yang ringan, kompatibel dengan SDK Sentry, dan bisa dijalankan sebagai satu binary dengan Postgres. Fokusnya hanya dua hal: **issues** dan **performance**. Fitur berat Sentry (replay, profiling, crons, dan sejenisnya) sengaja tidak dibuat.

## 2. Latar belakang dan masalah

- Sentry self-hosted membutuhkan banyak komponen (Relay, Kafka, ClickHouse, Snuba, Postgres, Redis, Symbolicator, worker) dan sumber daya besar (kira-kira 16 GB RAM; angka dapat berubah antar versi).
- Untuk tim kecil sampai menengah yang hanya butuh melihat error dan performa aplikasi, biaya operasional dan sumber daya itu tidak sebanding.
- Aplikasi yang dimonitor memakai stack campuran (Django, FastAPI, Go, Rust, React), sehingga solusi harus mendukung semuanya tanpa menulis SDK sendiri.

## 3. Tujuan

1. Menyediakan visibilitas error (issues) lintas stack dengan grouping yang berguna.
2. Menyediakan visibilitas performa (latensi, throughput, error rate, trace) lintas service.
3. Ringan dioperasikan: satu binary, satu database, footprint kecil.
4. Kompatibel dengan SDK Sentry resmi sehingga adopsi cukup dengan mengganti DSN.
5. Menyediakan MCP server agar agent AI dapat membaca issues dan data performance, lalu membantu menganalisis dan memperbaikinya.

## 4. Non-goals

- Session replay, profiling, crons, attachment, session health/release health.
- Symbolication native.
- Billing dan paket langganan. Kuota per tenant hanya batas teknis (rate limit), bukan paket komersial.
- Menyalin atau menjalankan kode server Sentry (berlisensi FSL, pembatasan penggunaan kompetitif).

## 5. Pengguna dan kebutuhan

**Persona utama: developer/DevOps di tim internal** yang memelihara aplikasi Django, FastAPI, Go, Rust, dan React.

Kebutuhan inti:
- Tahu segera kalau ada error baru atau error lama muncul lagi setelah rilis.
- Melihat endpoint mana yang lambat dan sejak rilis mana.
- Menelusuri satu request dari frontend ke backend dalam satu trace.
- Deploy dan operasikan tanpa tim infrastruktur khusus.

## 6. User stories

**Issues**
- Sebagai developer, saya ingin error yang sama dikelompokkan jadi satu issue agar tidak tenggelam dalam duplikat.
- Sebagai developer, saya ingin melihat stack trace, environment, release, dan waktu kejadian per issue.
- Sebagai developer, saya ingin menandai issue resolved atau ignored, dan issue dibuka lagi otomatis kalau muncul lagi.
- Sebagai developer, saya ingin menerima alert ketika ada issue baru atau regresi.
- Sebagai developer frontend, saya ingin stack trace React terbaca (ter-resolve lewat source map).

**Performance**
- Sebagai developer, saya ingin melihat p50/p95/p99, throughput, dan error rate per endpoint atau transaksi.
- Sebagai developer, saya ingin membandingkan performa antar release.
- Sebagai developer, saya ingin melihat waterfall trace lintas service (React, FastAPI, Go).
- Sebagai developer, saya ingin tahu query DB atau operasi mana yang paling lambat.

**Operasional**
- Sebagai admin, saya ingin mengelompokkan data secara berjenjang (tenant, workspace, project) agar tim atau pelanggan berbeda terpisah rapi.
- Sebagai admin, saya ingin data satu tenant tidak pernah terlihat oleh tenant lain.
- Sebagai admin, saya ingin membuat project dan mendapat DSN untuk tiap aplikasi.
- Sebagai admin, saya ingin mengatur retention dan rate limit per project.
- Sebagai admin, saya ingin data sensitif (password, token, header otorisasi) otomatis disamarkan.

**MCP / agent AI**
- Sebagai developer, saya ingin agent AI bisa mengambil daftar issue dan detailnya (stack trace, konteks kode, release) tanpa saya menyalin manual.
- Sebagai developer, saya ingin agent AI bisa melihat transaksi lambat dan trace-nya untuk mencari penyebab.
- Sebagai developer, saya ingin agent AI menandai issue resolved dengan catatan dan commit perbaikannya.
- Sebagai admin, saya ingin akses AI dibatasi token ber-scope, tercatat di audit log, dan aman dari instruksi berbahaya di dalam isi event.

## 7. Kebutuhan fungsional

Prioritas: **P0** = wajib MVP, **P1** = fase berikutnya, **P2** = nanti.

### 7.1 Ingest

| ID | Kebutuhan | Prioritas |
|---|---|---|
| F-ING-1 | Menerima envelope SDK Sentry dengan autentikasi DSN | P0 |
| F-ING-2 | Menerima body terkompresi (gzip, deflate, brotli) | P0 |
| F-ING-3 | Membuang item yang tidak didukung (session, replay, profile, attachment) | P0 |
| F-ING-4 | Rate limit per project dengan respons 429 dan header rate limit agar SDK mundur | P0 |
| F-ING-5 | Batas ukuran body dan event | P0 |
| F-ING-6 | Scrubbing PII yang dapat dikonfigurasi | P1 |

### 7.2 Issues

| ID | Kebutuhan | Prioritas |
|---|---|---|
| F-ISS-1 | Grouping otomatis berdasarkan fingerprint (tipe exception, frame in-app, fallback pesan) | P0 |
| F-ISS-2 | Daftar issue dengan count, first/last seen, filter project, environment, release, status | P0 |
| F-ISS-3 | Detail issue: stack trace, tag, environment, release, contoh event | P0 |
| F-ISS-4 | Status unresolved, resolved, ignored; reopen otomatis saat regresi | P0 |
| F-ISS-5 | Grafik kejadian per jam atau hari (dari tabel counter) | P0 |
| F-ISS-6 | Alert ke email, webhook, atau Telegram (issue baru, regresi, lonjakan) | P1 |
| F-ISS-7 | Release tracking | P1 |
| F-ISS-8 | Source map React: upload per release dan resolve saat ingest | P1 |
| F-ISS-9 | Merge dan split issue manual | P2 |
| F-ISS-10 | Breadcrumbs ditampilkan di detail event | P2 |

### 7.3 Performance

| ID | Kebutuhan | Prioritas |
|---|---|---|
| F-PRF-1 | Menerima transaksi dan span | P1 |
| F-PRF-2 | Agregasi per (project, transaksi, environment, release, menit): jumlah, error, histogram durasi | P1 |
| F-PRF-3 | Tampilan p50/p95/p99, throughput, error rate per transaksi | P1 |
| F-PRF-4 | Penyimpanan trace sampel (lambat, error, acak 1 sampai 5%) dan waterfall | P1 |
| F-PRF-5 | Distributed tracing lintas service (trace id, parent span id) | P1 |
| F-PRF-6 | Perbandingan antar release | P2 |
| F-PRF-7 | Agregasi query DB atau operasi paling lambat | P2 |

### 7.4 Administrasi

| ID | Kebutuhan | Prioritas |
|---|---|---|
| F-ADM-1 | Login pengguna (session cookie, password di-hash dengan argon2) | P0 |
| F-ADM-2 | Manajemen project dan DSN | P0 |
| F-ADM-3 | Retention default di tenant, dapat di-override per workspace dan project (retention global lewat partisi; yang lebih pendek lewat job hapus terjadwal) | P0 |
| F-ADM-4 | Token API per project | P1 |
| F-ADM-5 | Multi-pengguna dengan peran (`tenant_owner`, `tenant_admin`, `workspace_admin`, `member`, `viewer`); peran di tenant dan workspace, project mewarisi dari workspace | P1 |
| F-ADM-6 | Hierarki tenant → workspace → project; semua data dan query ter-scope ke tenant | P0 |
| F-ADM-7 | Isolasi data antar tenant (konteks tenant wajib di service layer, RLS Postgres sebagai lapisan tambahan) dan tes otomatis kebocoran lintas tenant | P0 |
| F-ADM-8 | Pengaturan turun: retention, scrubbing PII, kanal alert, dan default rate limit diwarisi dari tenant ke workspace ke project, dapat di-override | P1 |
| F-ADM-9 | Rate limit per project dan batas agregat per tenant | P1 |
| F-ADM-10 | Mode single-tenant: tenant dan workspace default dibuat otomatis, UI menyembunyikan tingkat tersebut sampai dibutuhkan | P0 |

### 7.5 MCP server

| ID | Kebutuhan | Prioritas |
|---|---|---|
| F-MCP-1 | MCP server di binary yang sama (Streamable HTTP, `/mcp`) di atas service layer yang dipakai UI | P1 |
| F-MCP-2 | Token Bearer per pengguna dengan scope akses (tenant, workspace, atau project) dan scope aksi `read` dan `triage`, dapat dicabut; default `read` | P1 |
| F-MCP-3 | Tool `list_projects` dan `list_issues` (filter project, environment, release, status, waktu, urutan) | P1 |
| F-MCP-4 | Tool `get_issue`: ringkasan, stack trace (default frame in-app), tag, release, konteks kode, event contoh | P1 |
| F-MCP-5 | Tool `get_event` dan `get_issue_stats` (time series) | P1 |
| F-MCP-6 | Tool `update_issue_status` (resolved, ignored, unresolved) dengan catatan dan referensi commit | P1 |
| F-MCP-7 | Tool `add_issue_note` | P1 |
| F-MCP-8 | Output hemat token: batas hasil, pagination, truncation dengan penanda, link ke UI | P1 |
| F-MCP-9 | Audit log semua panggilan MCP (siapa, tool, argumen, waktu) dan rate limit per token | P1 |
| F-MCP-10 | Konten event ditandai sebagai data tidak tepercaya dan PII di-scrub sebelum keluar (wajib sebelum MCP diaktifkan) | P1 |
| F-MCP-11 | Pemetaan release ke commit SHA (nama release berupa SHA atau dikonfigurasi per project) | P1 |
| F-MCP-12 | Tool `list_transactions` (p50/p95/p99, throughput, error rate; filter project, environment, release, waktu) | P1 (bersama fase 3) |
| F-MCP-13 | Tool `get_trace` (waterfall ringkas, span lambat ditandai) | P1 (bersama fase 3) |
| F-MCP-14 | Tool `list_slow_queries` dan `compare_releases` | P2 |
| F-MCP-15 | Prompt template MCP untuk triase dan perbaikan issue | P2 |

## 8. Kebutuhan non-fungsional

Angka di bawah adalah **target usulan** yang harus divalidasi lewat pengukuran, bukan hasil ukur.

- **Footprint:** aplikasi Rust puluhan MB RSS saat idle; seluruh stack (aplikasi + Postgres) mampu jalan di VM kecil.
- **Deployment:** satu binary + Postgres; UI React ter-embed di binary.
- **Throughput:** target ribuan envelope per detik di satu VM kecil, dengan batas di penulisan DB.
- **Latensi ingest:** respons cepat karena pemrosesan asinkron lewat channel terbatas.
- **Ketahanan:** saat beban berlebih, buang event dengan 429 (bukan menumbuhkan memori tanpa batas). Kehilangan beberapa detik data saat crash dapat diterima, kecuali spool file diaktifkan.
- **Storage:** terkendali lewat batas N event penuh per issue, trace sampel, dan retention lewat partisi.
- **Keamanan:** scrubbing PII, DSN per project, password ter-hash, TLS di depan aplikasi.
- **Keamanan MCP:** token ber-scope, tool tulis terbatas pada status dan catatan (tanpa hapus data atau ubah konfigurasi), audit log, rate limit, dan penandaan konten event sebagai data tidak tepercaya untuk mengurangi risiko prompt injection.
- **Kompatibilitas:** SDK Sentry resmi untuk Python (Django, FastAPI), Go, Rust, dan JavaScript/React.

## 9. Metrik keberhasilan

- Aplikasi internal (minimal satu Django atau FastAPI) berhasil melaporkan error ke Sentry-Lite hanya dengan mengganti DSN.
- Grouping: jumlah issue duplikat yang harus di-merge manual rendah (dinilai dari error nyata selama uji).
- Footprint dan throughput memenuhi target bagian 8 pada load test.
- Waktu dari error terjadi sampai alert diterima kurang dari 1 menit (setelah F-ISS-6).
- Stack trace React terbaca setelah source map di-upload (setelah F-ISS-8).
- Agent AI dapat mengambil satu issue lengkap (stack trace, konteks kode, release) lewat MCP dan menandainya resolved dengan catatan, tanpa membuka UI (setelah F-MCP-1..11).

## 10. Fase rilis

| Fase | Cakupan | Estimasi (satu developer, kasar) |
|---|---|---|
| 1 (MVP) | F-ING-1..5, F-ISS-1..5, F-ADM-1..3, F-ADM-6, F-ADM-7, F-ADM-10; uji dengan Django dan FastAPI | 3 sampai 5 minggu |
| 2 | F-ISS-6, F-ISS-7, F-ING-6 | 1 sampai 2 minggu |
| 2b | F-MCP-1..11 (tool issues dan triase) | sekitar 1 minggu |
| 3 | F-PRF-1..5, F-MCP-12..13 | 1 sampai 2 bulan (bersama fase 1 dan 2) |
| 4 | F-ISS-8 (source map React) | 1 sampai 2 minggu |
| 5 | Verifikasi Go dan Rust; F-ADM-4, F-ADM-5, F-ADM-8, F-ADM-9 | menyesuaikan |
| Nanti | P2 | - |

Versi matang: sekitar 3 sampai 4 bulan atau lebih. Seluruh estimasi adalah perkiraan kasar.

## 11. Asumsi, risiko, dan dependensi

**Asumsi**
- Protokol ingest Sentry cukup stabil dan terdokumentasi untuk diimplementasikan.
- Skala awal kecil sampai menengah; Postgres cukup tanpa ClickHouse.

**Risiko**

| Risiko | Mitigasi |
|---|---|
| Event storm | Rate limit per project, 429, sampling di SDK |
| Pertumbuhan storage | Retention dan partisi, batas event penuh per issue |
| Data sensitif di payload | Scrubbing di worker |
| Memori source map | LRU berbatas, parse lazy |
| Grouping buruk | Uji dengan error nyata, merge/split manual (P2) |
| Prompt injection lewat isi event ke agent AI | Tandai konten sebagai data tidak tepercaya, scope read default, tool tulis terbatas, audit log; klien tidak auto-approve perintah dari isi issue |
| Kebocoran data sensitif lewat MCP | Scrub PII sebelum keluar, token per project, rate limit |
| Kebocoran data lintas tenant | Konteks tenant wajib di service layer, RLS Postgres, tes isolasi otomatis termasuk jalur MCP |
| Retention pendek per tenant menimbulkan bloat | Batas retention maksimum sama dengan global, atau batch delete di jam sepi dengan pemantauan |
| Perubahan protokol atau SDK Sentry | Pin versi SDK yang diuji, uji kompatibilitas berkala |
| Lisensi | Implementasi dari spesifikasi protokol, bukan salinan kode server; cek lisensi terbaru sebelum rilis komersial |

**Dependensi**
- SDK Sentry resmi (Python, Go, Rust, JavaScript).
- Postgres.
- Crate Rust utama: axum, tokio, sqlx, serde_json, zstd, governor, sourcemap, argon2, tower-http.

## 12. Pertanyaan terbuka

1. Perkiraan volume event dan transaksi per hari?
2. Retention default (hari) untuk issue, event, dan trace?
3. (Terjawab) Hierarki tenant → workspace → project dipakai, mode single-tenant tetap didukung. Masih terbuka: tenant dibuat admin atau lewat pendaftaran mandiri (self-service)?
4. Apakah alternatif yang sudah ada (GlitchTip atau Bugsink untuk issues; SigNoz atau OpenTelemetry untuk performance) perlu dievaluasi lebih dulu sebelum membangun?
5. Kanal alert prioritas: email, webhook, atau Telegram?
6. Apakah ini murni alat internal atau direncanakan menjadi produk?
7. Scope MCP: cukup baca dan triase (status, catatan), atau perlu aksi lain?
8. Agent AI mana yang akan memakai MCP, dan apakah nama release sudah berupa commit SHA agar bisa dipetakan ke repo?
