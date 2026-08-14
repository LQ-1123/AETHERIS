# remote_pacs

**English** | [中文](README.zh-CN.md)

A self-hosted PACS (Picture Archiving and Communication System): Rust server + Tauri desktop viewer. Distributable, multi-account, shared platform database.

<p align="center">
  <img src="doc/演示/LOGO.png" alt="AETHERIS" width="200"/>
</p>

[![CI](https://github.com/LQ-1123/AETHERIS/actions/workflows/ci.yml/badge.svg)](https://github.com/LQ-1123/AETHERIS/actions/workflows/ci.yml)
[![Windows](https://github.com/LQ-1123/AETHERIS/actions/workflows/build-windows.yml/badge.svg)](https://github.com/LQ-1123/AETHERIS/actions/workflows/build-windows.yml)
[![Release](https://img.shields.io/github/v/release/LQ-1123/AETHERIS)](https://github.com/LQ-1123/AETHERIS/releases)
[![License](https://img.shields.io/github/license/LQ-1123/AETHERIS)](LICENSE)

## Download

Out-of-the-box installers (Viewer + server + embedded PostgreSQL, zero dependencies on the target machine):

| Platform | Installer | Notes |
|---|---|---|
| Windows x64 | [AETHERIS-Setup-0.1.0-x64.exe](https://github.com/LQ-1123/AETHERIS/releases/latest) | Inno Setup installer, double-click to install |
| macOS (Apple Silicon) | [AETHERIS_0.1.0_aarch64.dmg](https://github.com/LQ-1123/AETHERIS/releases/latest) | Double-click to run; auto-initializes local services and account |

> On macOS, if Gatekeeper shows "cannot verify the developer", right-click → Open (not notarized yet).
> Research/demo only — not clinically validated.

## Screenshots

| Login | Worklist | Patient search |
|---|---|---|
| ![Login](doc/演示/登陆.png) | ![Worklist](doc/演示/主界面1.png) | ![Search](doc/演示/查询.png) |

| MPR | Volume rendering | AI segmentation |
|---|---|---|
| ![MPR](doc/演示/MPR三维重建.png) | ![VR](doc/演示/VR重建.png) | ![AI mask](doc/演示/AI-mask标注.png) |

| Annotations | DICOM tag revision | Lifecycle |
|---|---|---|
| ![Annotations](doc/演示/标注.png) | ![Tag revision](doc/演示/DICOM-TAG修订.png) | ![Lifecycle](doc/演示/生命周期.png) |

| DICOM Router |
|---|
| ![Router](doc/演示/路由引擎.png) |

## What is it


A complete medical imaging archive and viewing stack written in Rust:

- **DIMSE server** (C-ECHO / C-STORE / C-FIND / C-MOVE / C-GET SCP) implemented from scratch
- **DICOMweb**: QIDO-RS / WADO-RS (STOW-RS in progress)
- **PostgreSQL** metadata store with byte-fidelity file archive
- **Tauri 2 desktop viewer**: 2D reading, MPR, MIP/MinIP, GPU volume rendering, measurement, shared annotations, 3D sparse masks, local AI segmentation
- **RBAC** auth (argon2, JWT + refresh tokens), audit log, worklists, versioned report amendments, lifecycle management (cold tiers, legal hold, quarantine)

## Structure

```
crates/
  pacs-core/    domain model, UID validation, DICOM metadata extraction
  pacs-store/   file persistence, fsync semantics, two-level hash-sharded paths
  pacs-db/      Postgres access layer, migrations, ingest transactions
  pacs-dimse/   from-scratch DIMSE services (C-ECHO/STORE/FIND/MOVE/GET SCP)
  pacs-auth/    accounts, argon2 hashing, tokens, RBAC, audit
  pacs-web/     axum: QIDO/WADO/STOW-RS + auth API
  pacs-codec/   pixel decoding, thumbnails, frame extraction
  pacs-ai/      local AI worker protocol, task cancellation, mask validation
  pacsd/        server entrypoint
apps/viewer/    Tauri 2 client (can also open local DICOM without a server)
```

Status: phases 0–4 complete; phase 5 QIDO-RS/WADO-RS read side complete, STOW-RS pending; phase 6 viewer supports local files and authenticated remote patient worklists.

## Quick start (Docker)

One command brings up the whole server stack — Postgres + pacsd + DCMTK device simulator (requires Docker with Compose v2; the Tauri viewer is a desktop app and runs on the host):

```sh
docker compose up -d --build   # first build compiles pacsd (release, ~10-20 min)
docker compose logs -f pacsd   # watch init: database → migrations → admin account
```

After startup:

- **HTTPS/DICOMweb**: `https://127.0.0.1:8443` (self-signed cert auto-generated at `data/docker-storage/tls/ca.crt`)
- **DIMSE**: `127.0.0.1:11112` — talk to it directly with `echoscu`/`storescu`
- **Device simulator**: `http://127.0.0.1:8787` — drag & drop DICOM folders; set device host to `pacsd`, port `11112`
- **Default admin**: `admin / pacs-demo-2026` (override via `.env`; also set `PACS_JWT_SECRET` for production)

Sample images:

```sh
./tools/fetch-sample-dicom.sh    # downloads public sample DICOMs to data/samples
```

Connect the viewer on the host:

```sh
cd apps/viewer && npm install && npm run tauri dev
```

Login with `https://127.0.0.1:8443` and CA cert `data/docker-storage/tls/ca.crt`. Local AI segmentation (lungmask) runs inside the viewer and never leaves the machine.

## Try it without Docker

Create an admin, start the server, then send images with DCMTK:

```sh
cargo run -p pacsd -- admin --username admin --password 'change-me'
cargo run -p pacsd

echoscu  -aet TEST_SCU -aec REMOTE_PACS 127.0.0.1 11112
storescu -aet TEST_SCU -aec REMOTE_PACS 127.0.0.1 11112 x.dcm
```

`echoscu` success means association and C-ECHO work; after `storescu` returns Success the image is persisted and indexed. Re-sending the same SOP Instance UID is idempotent.

### DCMTK multi-device simulator

`python3 tools/dcmtk-simulator.py` serves a UI at `http://127.0.0.1:8787`: drag in DICOM folders, configure multiple Calling/Called AE devices, and upload concurrently. Requires DCMTK (`brew install dcmtk` on macOS); port via `SIMULATOR_PORT`.

## Packaging & releases

**macOS (zero-dependency dmg)**: `npm run tauri build` assembles the local stack (pacsd + embedded PostgreSQL 14 + bundled libs, via `scripts/stage-local-stack.sh`), producing `apps/viewer/src-tauri/target/release/bundle/dmg/*.dmg`. Double-click to run: auto initdb, start services, create admin, auto-login.

**Windows (out-of-the-box exe)**: `.github/workflows/build-windows.yml` (manual trigger or `v*` tag) compiles pacsd + aetheris-launcher + viewer on windows-latest, fetches EDB PostgreSQL binaries and vcpkg libarchive, and assembles `AETHERIS-Setup-0.1.0-x64.exe` with Inno Setup. `initialize.ps1` handles initdb/database/admin on install; the launcher starts everything with one click.

## Development

Requires Rust 1.97.1 (pinned by `rust-toolchain.toml`), PostgreSQL, DCMTK.

```sh
cargo build --workspace
cargo test --workspace
cargo clippy --workspace --all-targets -- -D warnings
```

### API checker

Open `https://127.0.0.1:8443/api-checker` after starting `pacsd`. It reads `/api/v1/openapi.json`, merges DICOMweb/viewer/annotation/segmentation/transfer routes, and supports login, single-request testing, auth-protection batch scans, GET smoke tests, and JSON export. Batch scans never auto-execute write endpoints.

### Viewer

Multi-frame single files and multi-file grayscale series are sorted strictly by `ImagePositionPatient`/`ImageOrientationPatient`; without reliable geometry the viewer refuses to open rather than guessing from filenames. Mixed localizers or sizes are split into independent image groups (main stack first, switchable in the top-right of the viewport).

```sh
cd apps/viewer
npm install
npm run build
npm test
npm run tauri dev
```

Enable lightweight local lung segmentation:

```sh
./ai-worker/setup.sh
npm run tauri dev
```

Default model is `lungmask R231` (~119 MB, downloaded on first inference). Apple Silicon uses MPS automatically; DICOM is read only by the local worker. Override with `PACS_AI_PYTHON` / `PACS_AI_WORKER`.

Tools: window/level, cursor-anchored zoom, pan, series navigation, window presets, and two-point measurement (distinguishing calibrated mm, detector-plane mm, and pixel-only results). Scroll switches frames, `Ctrl + scroll` zooms, middle-drag pans.

### Database

The server exclusively owns database connections; clients never connect directly. Connection string comes from `.env`:

```sh
cp .env.example .env   # then fill in real credentials
```

Migrations are embedded in the binary and applied automatically at startup — no SQL files to ship at deploy time.

### Testing

- `pacs-db` integration tests run against a real Postgres (those SQL queries can only be verified on a real database)
- `pacsd` interop tests start the server and drive it with real DCMTK `echoscu`/`storescu` traffic (DCMTK is the de-facto interop benchmark)

Both need `PACS_TEST_DATABASE_URL`; interop tests also need DCMTK. Tests skip with a notice when prerequisites are missing — but skip in CI means failure, so CI never goes green without actually testing. The test database is created automatically.

### Benchmark

```sh
cargo run --release -p pacsd --example bench_ingest -- 200 8 512
#                                                      files concurrency size
```

Measures the path that must complete before C-STORE returns Success: parse → fsync to disk → transaction commit. Use `--release` (debug DICOM parsing is an order of magnitude slower).

## Design notes

Key invariants — read the corresponding plan chapters before changing anything:

- **Clients never connect to the database directly.** Embedding a connection string in distributed clients would hand every user the DB credentials — no permission control, revocation, or rotation.
- **UIDs are validated before ingest.** UIDs are used as path components and come from external devices; `pacs_core::Uid` guarantees constructed values are safe single-level path names.
- **C-STORE must not return Success before data is actually durable.** Order: write temp file → fsync → rename → fsync parent dir → DB commit → only then return `0x0000`. Devices genuinely delete their local copy after a success response.
- **Command sets are always Implicit VR Little Endian**, independent of the negotiated transfer syntax (PS3.7 §6.3.1). Decoding command sets per the negotiated syntax garbles explicit-VR connections.
- **Stored dataset bytes are byte-identical to what the sender wrote.** Only a file-metadata prefix is prepended; no decode/re-encode round-trip.
- **CT series ordering must not use `InstanceNumber`.** Sort by the projection of `ImagePositionPatient` onto the slice normal.

## Security notes

- DIMSE has no authentication (AE titles are forgeable); HTTP reads use TLS, accounts, and permissions. The server binds `127.0.0.1` by default and warns loudly when configured otherwise.
- The current self-signed certificate covers loopback only — don't switch to LAN/public listening at this stage. Real devices require proper SANs, network access control, and device allowlists.
- Real patient data implicates HIPAA / GDPR / PIPL compliance.

## License

[MIT](LICENSE)
