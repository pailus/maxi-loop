# Sentry-Lite: Dokumen Desain

Status: draft awal (2026-11-21)
Server: Rust | UI: React | Database: Postgres

## 1. Tujuan

Membangun aplikasi mirip Sentry yang ringan, hanya untuk dua kebutuhan inti:

1. **Issues**: error tracking dengan grouping, status, dan alert.
2. **Performance**: transaksi dan span, percentile (p50/p95/p99), throughput, error rate, dan trace waterfall.

Alasan: Sentry self-hosted berat (belasan container: Relay, Kafka, ClickHouse, Snuba, Postgres, Redis, Symbolicator, banyak worker; kebutuhan resminya kira-kira 16 GB RAM, angka bisa berubah antar versi). Untuk skala tim atau beberapa produk, satu binary dengan Postgres sudah cukup.

## 2. Stack yang dimonitor

Django, FastAPI, Go, Rust, React.

| Stack | Kesulitan | Catatan |
|---|---|---|
| Django, FastAPI | Mudah | Integrasi matang; stack trace membawa potongan source; transaksi dan span otomatis. Mulai dari sini. |
| Go | Cukup mudah | Perlu middleware (net/http, gin, echo, fiber) dan recover untuk panic. Tracing lebih manual, span lebih sedikit. |
| Rust | Menengah | Crate `sentry` + integrasi `tracing`. Build release yang di-strip menghilangkan nama file/baris; simpan debug info. Nama fungsi ter-mangle. |
| React | Sulit | Kode produksi di-minify, wajib source map per release. Data performance browser (page load, web vitals, fetch) dikirim SDK. |

## 3. Non-goals

Session replay, profiling, crons, attachment, symbolication native, serta billing atau paket langganan. Isolasi multi-tenant masuk scope (lihat 5.11); kuota per tenant hanya batas teknis (rate limit).

## 4. Strategi kompatibilitas: pakai SDK Sentry yang ada

Server mengimplementasikan protokol ingest Sentry (endpoint envelope dengan autentikasi DSN), sehingga SDK resmi untuk semua stack di atas bisa dipakai tanpa menulis SDK sendiri. Pendekatan yang sama dipakai GlitchTip dan Bugsink.

Catatan lisensi: SDK Sentry open source, tetapi kode server Sentry berlisensi FSL yang melarang penggunaan kompetitif. Implementasikan dari spesifikasi protokol, jangan menyalin kode server. Cek ulang lisensi terbaru sebelum rilis komersial.

## 5. Arsitektur

Prinsip: satu binary, satu database, tanpa Kafka, ClickHouse, dan Redis. Ringan karena hot path tipis, penulisan ke DB selalu batch, dan data mentah tidak disimpan semuanya.

```
SDK ──► [ axum ingest ] ──► channel terbatas ──► [ worker pool ] ──► Postgres
          auth, limit,        (backpressure)       fingerprint,       (partisi harian)
          buang item          tokio mpsc           scrub, batch insert
          yang tak dipakai
                                                  ▲
        [ scheduler: alert, rollup, retention ] ──┘
        [ API JSON + React build ter-embed di binary yang sama ]
```

### 5.1 Ingest

- Endpoint envelope: validasi DSN, batasi ukuran body, terima body terkompresi (gzip, deflate, brotli).
- Buang item yang tidak didukung (session, replay, profile, attachment) sedini mungkin.
- Jangan deserialize seluruh event ke struct besar. Ambil hanya field yang dibutuhkan (exception, stack frame, trace id, timestamp) dan simpan sisanya sebagai bytes mentah.
- Rate limit per project dengan token bucket di memori (`governor`). Balas 429 dengan header rate limit agar SDK mundur sendiri.
- Masukkan ke channel `tokio::mpsc` yang terbatas, lalu balas segera.

### 5.2 Worker dan batch insert

- Worker menormalisasi event, scrub PII, menghitung fingerprint, dan upsert issue.
- Kumpulkan event selama 100 ms sampai 1 detik lalu tulis sekali (multi-row insert atau COPY). Ini penghematan CPU dan I/O terbesar.
- Kalau channel penuh: buang event dan balas 429. Kehilangan beberapa detik data saat crash masih dapat diterima untuk error tracking; kalau tidak, tambahkan spool file lokal.

### 5.3 Grouping (fingerprint)

- Hash dari tipe exception dan frame stack trace yang in-app (modul dan fungsi), dengan fallback ke pesan error.
- Untuk JavaScript, grouping memakai frame yang sudah di-resolve lewat source map, jadi resolve dilakukan saat event masuk.
- Kualitas grouping menentukan kenyamanan pakai. Grouping yang buruk menghasilkan issue duplikat.

### 5.4 Penyimpanan issues

- Tabel issue: count, first seen, last seen, status (unresolved, resolved, ignored), regression (reopen otomatis).
- Simpan payload lengkap hanya untuk N event pertama per issue (misalnya 50) ditambah sampel. Event berikutnya cukup menaikkan counter, dan grafik berasal dari tabel counter per jam.
- Satu bug yang meledak menjadi jutaan event tidak membengkakkan storage.

### 5.5 Performance lewat agregasi

- Per (project, nama transaksi, environment, release, bucket 1 menit) simpan jumlah, jumlah error, dan histogram durasi (DDSketch atau histogram log-scale). Percentile dihitung dari histogram.
- Trace lengkap hanya disimpan untuk sampel: yang lambat, yang error, dan 1 sampai 5% acak.
- Span satu trace disimpan sebagai satu blob terkompresi (zstd), bukan satu baris per span.
- Query DB paling lambat dan operasi lain diagregasi per operasi dan deskripsi yang sudah dinormalisasi.
- Distributed tracing: simpan `trace_id` dan `parent_span_id` sehingga satu waterfall bisa menampilkan alur React, FastAPI, dan service Go.

### 5.6 Retention

Partisi tabel event dan trace per hari atau minggu. Penghapusan data lama dengan `DROP PARTITION` (instan, tanpa bloat). Retention default ditetapkan di tenant dan dapat di-override per workspace atau project; yang lebih pendek dari retention global memakai job hapus terjadwal (lihat 5.11).

### 5.7 Source map

- Upload per release (matching release + nama file, atau debug id).
- Resolve saat event masuk, dengan cache hasil parse dalam LRU berbatas memori. Parse source map besar adalah risiko memori utama.
- Crate: `sourcemap`.

### 5.8 Satu proses

Alert (cek counter tiap menit), rollup, dan manajemen partisi berjalan sebagai task tokio di binary yang sama, dengan flag untuk memisahkan peran kalau nanti perlu. React build disajikan lewat axum (`rust-embed`). Autentikasi UI: session cookie dengan `argon2`; token API per project.

### 5.9 Crate utama

`axum`, `tokio`, `sqlx`, `serde_json`, `zstd`, `governor`, `sourcemap`, `argon2`, `tower-http`, `tracing`, `rust-embed`, `rmcp` (SDK Rust untuk MCP; cek versi terbaru).

### 5.10 MCP server (akses untuk AI agent)

Tujuan: agent AI (misalnya Claude Code) bisa membaca issue dan data performance, menelusuri penyebabnya di repo, memperbaiki kode, lalu menandai issue sebagai resolved.

**Bentuk.** MCP server berjalan di binary yang sama (transport Streamable HTTP, path `/mcp`), sebagai lapisan tipis di atas service layer internal yang juga dipakai API untuk UI. Jangan menulis query terpisah untuk MCP. Opsional: subcommand `sentry-lite mcp` (stdio) yang meneruskan ke API untuk klien yang hanya mendukung stdio.

**Tool (nama usulan).**

| Tool | Fungsi | Scope |
|---|---|---|
| `list_projects` | Daftar project | read |
| `list_issues` | Filter project, environment, release, status, rentang waktu, urutan (terbaru, terbanyak, regresi) | read |
| `get_issue` | Ringkasan, stack trace (default hanya frame in-app), tag, release, konteks kode, satu event contoh | read |
| `get_event` | Detail satu event (breadcrumbs, request, user yang sudah di-scrub) | read |
| `get_issue_stats` | Time series kejadian | read |
| `list_transactions` | p50/p95/p99, throughput, error rate per transaksi | read |
| `get_trace` | Waterfall ringkas, span paling lambat ditandai | read |
| `list_slow_queries`, `compare_releases` | Query DB paling lambat; perbandingan antar release | read (tahap lanjut) |
| `update_issue_status` | resolved, ignored, unresolved, dengan catatan dan referensi commit | triage |
| `add_issue_note` | Menambah catatan analisis ke issue | triage |

**Agar agent benar-benar bisa mengerjakan, bukan hanya membaca:**
- Sertakan path file, nomor baris, dan potongan kode sekitar (`pre_context`, `context_line`, `post_context` dari SDK) di `get_issue`, supaya mudah dipetakan ke repo.
- Petakan release ke commit SHA (kalau nama release berupa SHA atau dikonfigurasi di project), supaya agent tahu versi kode yang bermasalah.
- Output hemat token: batas jumlah hasil, pagination, truncation dengan penanda yang jelas, dan link ke halaman UI.
- Alur khas: `list_issues` → `get_issue` → checkout commit rilis di repo → perbaiki dan uji → `update_issue_status` dengan catatan dan commit.

**Keamanan (penting).**
- Isi event (pesan error, URL, header, input pengguna) dikendalikan pihak luar dan bisa berisi instruksi yang menyasar agent (prompt injection). Bungkus konten event dengan penanda yang jelas sebagai data tidak tepercaya, dan scrub PII sebelum keluar lewat MCP.
- Token Bearer per pengguna dengan scope akses (tenant, workspace, atau project) dan scope aksi `read` dan `triage`, dapat dicabut. Default `read` saja.
- Tool tulis dibatasi pada status dan catatan. Tidak ada tool untuk menghapus data atau mengubah konfigurasi.
- Audit log semua panggilan (siapa, tool, argumen, waktu) dan rate limit per token.
- Di sisi klien, jangan auto-approve eksekusi perintah yang berasal dari isi issue.

**Dampak performa.** Query MCP membaca tabel agregat dan counter, bukan raw data, dan tidak berada di jalur ingest. Batasi ukuran hasil dan jumlah panggilan per token agar tidak membebani Postgres.

### 5.11 Hierarki tenant, workspace, dan project

```
Tenant (organisasi atau pelanggan)
 └── Workspace (tim atau lingkungan, mis. "Produk A", "Internal")
      └── Project (satu aplikasi atau service, punya DSN)
```

- **Skema:** tabel `tenants`, `workspaces` (tenant_id), `projects` (workspace_id, tenant_id), dan `memberships` (pengguna, scope tenant atau workspace, peran). Semua tabel data (issue, event, counter, transaksi, trace, release, source map, alert) membawa `tenant_id` dan `project_id`, ditaruh di depan primary key dan indeks.
- **Ingest:** DSN dipetakan ke (project, workspace, tenant) dari cache memori yang di-refresh saat konfigurasi berubah. Tidak ada query DB per envelope.
- **Isolasi data:** setiap query lewat service layer yang mewajibkan konteks tenant. Lapisan kedua: Row-Level Security Postgres (set `app.tenant_id` per transaksi, hati-hati dengan connection pooling). Worker batch yang menulis lintas project memakai role khusus dan tetap mengisi `tenant_id` dari cache DSN. Tambahkan tes otomatis yang memastikan pengguna tenant A tidak dapat membaca data tenant B, termasuk lewat MCP.
- **Peran:** `tenant_owner`, `tenant_admin`, `workspace_admin`, `member`, `viewer`. Peran di tenant berlaku untuk semua workspace; peran di workspace berlaku untuk semua project di dalamnya. Belum ada peran per project di awal.
- **Pengaturan turun (inheritance):** retention, scrubbing PII, kanal alert, dan default rate limit ditetapkan di tenant dan dapat di-override di workspace atau project.
- **Rate limit:** token bucket per project, ditambah batas agregat per tenant agar satu tenant tidak menghabiskan kapasitas.
- **Partisi tetap per waktu, bukan per tenant**, supaya jumlah partisi terkendali. Konsekuensinya, retention global dilakukan dengan `DROP PARTITION`, sedangkan retention tenant atau project yang lebih pendek dari global memakai job hapus terjadwal (batch delete, tidak instan dan menimbulkan bloat; jadwalkan di jam sepi dan pantau). Alternatif sederhana: batasi retention maksimum yang boleh dipilih sama dengan retention global.
- **Mode single-tenant:** instalasi baru membuat satu tenant dan workspace default secara otomatis, dan UI menyembunyikan tingkat tenant dan workspace sampai dibutuhkan. Menambahkan `tenant_id` sejak awal murah; menambahkannya belakangan sangat mahal (migrasi semua tabel, indeks, dan query).
- **URL dan API:** berbasis slug, misalnya `/t/{tenant}/w/{workspace}/p/{project}`. Token API dan token MCP dapat dibatasi ke tenant, workspace, atau project.
- **Dampak jadwal:** menambah kira-kira 1 sampai 2 minggu pada fase 1 (auth, RBAC, scoping query, uji isolasi). Perkiraan kasar.

## 6. Jalur skala (nanti, jangan dibangun sekarang)

1. Pecah peran ingest dan worker dengan binary yang sama.
2. Tambah antrean (misalnya NATS) di antaranya.
3. Pindahkan tabel event dan span ke ClickHouse di balik trait storage.

Jaga batas modul tetap rapi sejak awal supaya langkah ini murah.

## 7. Risiko dan jebakan

| Risiko | Mitigasi |
|---|---|
| Event storm | Rate limit per project, header 429, sampling di SDK |
| Pertumbuhan storage | Retention dan partisi sejak awal, batas N event penuh per issue |
| Data sensitif di payload | Scrub PII (password, token, header otorisasi) di worker |
| Memori source map | LRU berbatas dan parse lazy |
| Grouping buruk | Uji dengan error nyata dari aplikasi sendiri, sediakan merge dan split issue |
| Throughput DB | Batch insert, load test sejak tahap 1 |
| Prompt injection lewat isi event ke agent AI | Tandai konten sebagai data tidak tepercaya, scope read default, tool tulis terbatas, audit log |
| Kebocoran data sensitif lewat MCP | Scrub PII sebelum keluar, token per project, rate limit |
| Kebocoran data lintas tenant | Konteks tenant wajib di service layer, RLS Postgres, tes isolasi otomatis termasuk jalur MCP |
| Retention pendek per tenant menimbulkan bloat | Batas retention maksimum sama dengan global, atau batch delete di jam sepi dengan pemantauan |

## 8. Fase dan estimasi (satu developer, kasar)

| Fase | Isi | Estimasi |
|---|---|---|
| 1 | Endpoint envelope, tabel event dan issue, grouping, UI daftar issue, hierarki tenant, workspace, project dengan isolasi; uji dengan Django dan FastAPI | 3 sampai 5 minggu |
| 2 | Alert (email, webhook, Telegram) dan release tracking | 1 sampai 2 minggu |
| 2b | MCP server: tool issues (baca) dan triase (status, catatan); tool performance menyusul bersama fase 3 | sekitar 1 minggu (karena hanya lapisan tipis di atas API) |
| 3 | Transaksi dan span, percentile per endpoint, waterfall | 1 sampai 2 bulan (bersama fase 1 dan 2) |
| 4 | Source map untuk React | 1 sampai 2 minggu |
| 5 | Verifikasi Go dan Rust (stack trace dan tracing terbaca baik) | menyesuaikan |

Versi matang (multi-user dan project, retention, rate limiting, scrubbing, polesan UI): sekitar 3 sampai 4 bulan atau lebih. Semua angka di atas perkiraan kasar, bukan hasil ukur.

## 9. Target performa (perlu diukur)

- Aplikasi Rust: puluhan MB RSS saat idle (perkiraan).
- Throughput di satu VM kecil: kemungkinan ribuan envelope per detik, dengan batas di penulisan DB, bukan di Rust (perkiraan).
- Lakukan load test sejak fase 1 dengan generator envelope palsu (misalnya `oha` atau k6) dan catat hasilnya.

## 10. Pertanyaan terbuka

- Perkiraan volume event dan transaksi per hari.
- Retention default (hari) untuk issue, event, dan trace.
- Tenant dibuat admin atau lewat pendaftaran mandiri (self-service)? Hierarki tenant, workspace, project sudah diputuskan (lihat 5.11).
- Perlu tidaknya alternatif yang sudah ada dievaluasi dulu: GlitchTip atau Bugsink untuk issues, SigNoz atau OpenTelemetry untuk performance.
- Scope MCP: cukup baca dan triase (status, catatan), atau perlu aksi lain?
- Agent AI mana yang akan memakai MCP, dan apakah repo kode sudah terhubung ke release (nama release = commit SHA)?

## 11. Langkah berikutnya

1. Skema Postgres: issues, events, counter per jam, bucket performance, trace, partisi.
2. Kerangka handler envelope di axum (auth DSN, dekompresi, parsing item, channel).
3. Generator envelope untuk load test.
