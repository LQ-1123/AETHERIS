
# AETHERIS

<p align="center">
  <img src="./logo.jpg" width="128" alt="AETHERIS Logo">
</p>

<h3 align="center">A Modern, Self-Hosted Medical Imaging Infrastructure</h3>

<p align="center">
  <strong>Built from the ground up with Rust.</strong><br>
  DICOM · DICOMweb · PACS · 2D/3D Visualization · Local AI · Secure Workflows
</p>

<p align="center">
  <a href="https://github.com/LQ-1123/AETHERIS">GitHub</a>
  ·
  <a href="https://github.com/LQ-1123/AETHERIS/issues">Issues</a>
  ·
  <a href="https://github.com/LQ-1123/AETHERIS/releases">Releases</a>
</p>

<p align="center">

![Rust](https://img.shields.io/badge/Rust-1.97%2B-orange?logo=rust)
![Tauri](https://img.shields.io/badge/Tauri-2-24C8DB?logo=tauri)
![PostgreSQL](https://img.shields.io/badge/PostgreSQL-14%2B-336791?logo=postgresql)
![DICOM](<https://img.shields.io/badge/DICOM-DIMSE%20%7C%20DICOMweb-0B6E99>)
![License](https://img.shields.io/badge/License-MIT-green)
![Platform](<https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-lightgrey>)

</p>

[English](README.md) · [中文](README.zh-CN.md)

---

## Overview

**AETHERIS** is a self-hosted medical imaging infrastructure designed around a simple idea:

> **Medical imaging should be interoperable, durable, observable, and locally controlled.**

Instead of treating PACS as a collection of legacy services, AETHERIS approaches the problem as a modern software system — with a Rust-based core, explicit storage guarantees, standards-oriented networking, a native desktop viewer, and local AI capabilities.

The platform combines:

* **DICOM networking** through DIMSE
* **Modern HTTP interoperability** through DICOMweb
* **Durable medical image storage**
* **PostgreSQL-backed metadata indexing**
* **Native desktop visualization**
* **MPR / MIP / MinIP / volume rendering**
* **Measurement and annotation**
* **Local AI segmentation**
* **RBAC, authentication and audit logging**
* **Server-side patient worklists and exam requests**
* **Structured reporting and institution-controlled peer review**
* **Account, password-reset and workload administration**
* **Docker-based deployment**
* **Windows and macOS distributable applications**

AETHERIS is intended to be both a usable PACS platform and an engineering foundation for future intelligent medical imaging workflows.

> **Research / engineering project. Not clinically validated. Not intended for diagnosis or direct clinical decision-making.**

---

## Latest Release — v0.3.0

v0.3.0 connects the patient queue, image review, exam requests, structured reporting, independent report review, account administration, and workload reporting into one institution-scoped workflow.

| Platform | Download | SHA-256 |
| --- | --- | --- |
| macOS Apple Silicon | [AETHERIS_0.3.0_aarch64.dmg](https://github.com/LQ-1123/AETHERIS/releases/download/v0.3.0/AETHERIS_0.3.0_aarch64.dmg) | `f454761759d07acca4bccaf9d0a1af447425a9edaaebbf12c98719d8782dfa78` |
| Windows 10/11 x64 | [AETHERIS-Setup-0.3.0-x64.exe](https://github.com/LQ-1123/AETHERIS/releases/download/v0.3.0/AETHERIS-Setup-0.3.0-x64.exe) | `a926a5c479071f6b9d41722fa3a9c6915047e3b733ce98e86e198df4b614ea67` |

See the [v0.3.0 release notes](doc/releases/v0.3.0.md) for the full feature list, upgrade procedure, validation results, and known limitations.

---

## Screenshots

| Login                               | Main Workspace (Worklist + Viewer)                  | Multi-window Viewing                              |
| ----------------------------------- | --------------------------------------------------- | ------------------------------------------------- |
| ![Login](doc/screenshots/login.png) | ![Main Workspace](doc/img/%E6%9B%B4%E6%96%B0%E7%9A%84%E4%B8%BB%E7%95%8C%E9%9D%A2.png) | ![Multi-window Viewing](doc/img/%E5%A4%9A%E7%AA%97%E5%8F%A3%E5%9B%BE%E5%83%8F.png) |

| MPR Reconstruction                             | Volume Rendering                                          | AI Segmentation                                         |
| ---------------------------------------------- | --------------------------------------------------------- | ------------------------------------------------------- |
| ![MPR Reconstruction](doc/img/%E5%A4%9A%E8%A7%92%E5%BA%A6MPR%E9%87%8D%E5%BB%BA.png) | ![Volume Rendering](doc/screenshots/volume-rendering.png) | ![AI Segmentation](doc/screenshots/ai-segmentation.png) |

| Report Workbench (Radiologist)                 | Annotations                                     | DICOM Tag Revision                                |
| ---------------------------------------------- | ----------------------------------------------- | ------------------------------------------------- |
| ![Report Workbench](doc/img/%E6%8A%A5%E5%91%8A%E4%B9%A6%E5%86%99%E7%95%8C%E9%9D%A2-%E5%8C%BB%E7%94%9F.png) | ![Annotations](doc/screenshots/annotations.png) | ![Tag Revision](doc/screenshots/tag-revision.png) |

| Admin Console                          | Lifecycle                                   | DICOM Router                          |
| -------------------------------------- | ------------------------------------------- | ------------------------------------- |
| ![Admin Console](doc/img/%E7%AE%A1%E7%90%86%E6%8E%A7%E5%88%B6%E5%8F%B0.png) | ![Lifecycle](doc/screenshots/lifecycle.png) | ![Router](doc/screenshots/router.png) |

---

## Why AETHERIS?

Traditional PACS deployments often involve a collection of tightly coupled systems, vendor-specific configurations, and infrastructure that is difficult to reproduce outside a hospital environment.

AETHERIS takes a different approach:

<p align="center"><img src="doc/diagrams/why-aetheris.svg" alt="AETHERIS platform architecture" width="620"/></p>

The goal is not simply to make another DICOM viewer.

The goal is to build a **complete, composable medical imaging infrastructure**.

---

# Core Capabilities

## DICOM Networking

AETHERIS implements the core DIMSE services required for PACS interoperability:

| Service     | Status |
| ----------- | ------ |
| C-ECHO SCP  | ✅     |
| C-STORE SCP | ✅     |
| C-FIND SCP  | ✅     |
| C-MOVE SCP  | 🚧     |
| C-GET SCP   | 🚧     |

> C-MOVE / C-GET SCP are on the roadmap but **not yet implemented**; incoming requests for them are currently rejected (association aborted). Retrieval today goes through WADO-RS or C-STORE.

The DIMSE layer is implemented in the Rust workspace rather than relying entirely on a third-party PACS server.

This makes the protocol layer explicit, testable, and extensible.

---

## DICOMweb

Modern HTTP-based interoperability is provided through DICOMweb:

| Standard | Status            |
| -------- | ----------------- |
| QIDO-RS  | ✅                |
| WADO-RS  | ✅                |
| STOW-RS  | ✅ Part10 · 🚧 DICOM JSON variant |

DICOMweb provides a clean bridge between traditional modality infrastructure and modern web-based applications.

---

## Durable Storage

AETHERIS treats image persistence as a correctness problem rather than simply a file-copy operation.

The C-STORE path follows:

<p align="center"><img src="doc/diagrams/cstore-durability.svg" alt="C-STORE durability path" width="620"/></p>

The server does **not** report success before the received object has reached durable storage.

Stored DICOM datasets preserve the original byte content rather than performing an unnecessary decode → re-encode cycle.

---

# Native Medical Imaging Viewer

AETHERIS includes a native desktop viewer built with **Tauri 2**.

The viewer can operate both as a remote PACS client and as a local DICOM viewer.

### 2D Visualization

* Window / Level
* Window presets
* Zoom
* Pan
* Series navigation
* Multi-frame support
* Multi-file series
* Image measurement
* Annotations
* Multi-window split-screen viewing

### Multi-window Split-screen Viewing

Drag a series row from **Patient → Study → Series** into the workspace to split the screen automatically and render multiple series side by side:

* 1 series: 1×1
* 2 series: 1×2
* 3–4 series: 2×2
* 5–6 series: 3×2
* 7–9 series: 3×3

Each pane has its own `Renderer` and `ViewState` for independent navigation, windowing, zoom/pan, measurement, and annotations. Click a pane to activate it; hold `Alt` while dropping onto an occupied pane to replace that series; use the pane close button to remove it. Multi-pane mode currently focuses on 2D comparison reading; MPR / VR run in single-pane mode.

<p align="center"><img src="doc/img/多窗口图像.png" alt="Multi-window split-screen viewing" width="760"/></p>

### Geometry-aware Series Reconstruction

Series ordering does not rely on filenames or `InstanceNumber`.

AETHERIS uses:

<p align="center"><img src="doc/diagrams/series-reconstruction.svg" alt="Geometry-aware series reconstruction" width="620"/></p>

When reliable geometry cannot be established, the viewer refuses to guess.

This is intentional.

> **In medical imaging, a plausible image in the wrong order is worse than an explicit failure.**

---

# Advanced Visualization

AETHERIS is designed beyond basic 2D image viewing.

Current visualization capabilities include:

* MPR — Multiplanar Reconstruction
* MIP — Maximum Intensity Projection
* MinIP — Minimum Intensity Projection
* GPU-accelerated volume rendering
* 3D sparse masks
* Interactive measurement
* Annotation overlays

### GPU Oblique MPR (arbitrary-angle multiplanar reconstruction)

Entering MPR automatically loads the GPU volume and turns the Axial / Coronal / Sagittal viewports into patient-space linked oblique reconstructions:

* The crosshair lines in each viewport are the reference lines; drag a crosshair arm away from its center to rotate it.
* Rotating one viewport rebuilds the other two in real time; all three planes remain orthogonal and pass through the same patient-space center point.
* Double-click any viewport to restore standard Axial / Coronal / Sagittal orientation.
* The cursor changes to a rotation indicator near crosshair lines to prevent accidental interaction.
* Each viewport shows a cube/plane intersection icon in the top-right corner, dynamic `R / L / A / P / S / I` labels on the edges, and tilt angle plus DICOM `Image Orientation (Patient)` direction cosines in the top-left.

Geometry foundation:

* All MPR plane calculations happen in Patient Space using 4×4 affine transforms derived from `ImageOrientationPatient`, `ImagePositionPatient`, `PixelSpacing`, and `Spacing Between Slices`.
* Oblique planes compute independent `spacingX` / `spacingY` and a physical FOV/output size based on the plane-volume intersection, supporting anisotropic voxels.
* MPR / MIP / MinIP resample the GPU 3D texture in real time; when 16-bit textures are unavailable, an RG8 dual-channel fallback preserves HU precision.
* Length and angle measurements use true physical spacing so the same anatomy stays consistent across MPR orientations.

<p align="center"><img src="doc/img/多角度MPR重建.png" alt="Multi-angle MPR reconstruction" width="760"/></p>

Volume rendering mouse gestures:

* Left drag — Window / Level
* Right drag — Rotate
* Middle drag — Pan
* Wheel — Zoom

The architecture is intended to support progressively more advanced volumetric visualization without coupling the viewer to the server implementation.

---

# Clinical Workflow

## Patient Queue and Exam Requests

The full-screen patient queue is the default landing page after login. It presents one row per study with server-side pagination, sorting, and combined filters for source institution, report status, modality, body part, and date.

Technicians and administrators can manage exam requests in either direction:

* Create the request first, receive the images later, and manually bind the matching study.
* Create a request directly from an already-ingested study; patient and Study data are read by the server instead of trusted from the client.

Exam requests carry modality, body part, request type, and clinical indication through the `pending → executed → completed` lifecycle. The request is visible in the report workspace and is completed automatically when the associated report is approved or signed.

## Separate Report Workspace

Diagnostic reporting runs in a separate desktop window so the Viewer remains focused on image display and interaction. Reports are study-scoped and support:

* Structured findings, impression, recommendation, and positive-result fields
* Draft saving and immutable signed version snapshots
* Submit-for-review, review claim, reviewer correction, approval, and post-sign amendment
* Patient/institution context, exam-request indication, reviewer identity, and a complete review timeline

## Institution-Controlled Peer Review

Administrators can enable or disable the report-review closure under **Admin Console → Institution Settings**. The setting is persisted per institution and takes effect immediately.

When enabled, authors can save drafts and submit them for review but cannot sign directly. Reviewers need the `review_report` permission and cannot review their own reports. Reviewer corrections preserve the original author, identify the reviewer, and create a `reviewer_modified` audit event that contributes to workload/error statistics. When disabled, the direct draft-to-sign path remains available for single-radiologist and demonstration environments.

## Administration and Workload

The Admin Console provides account creation, first-login password change, enable/disable with session revocation, reviewer permission grants, device registration, source ownership, user access grants, institution settings, and per-user workload reports.

Password reset is approval-based: a user submits a username and proposed new password from the login screen, only an Argon2id hash is stored, and an administrator can approve or reject the request without seeing the password. Workload reports aggregate report states, signed versions, completed reviews, reviewer modifications, and exam requests over a selected date range.

All account, report, device, exam-request, workload, and institution-setting boundaries are enforced by the server rather than by hidden UI controls.

---

# Local AI

AETHERIS includes a local AI worker architecture for medical image processing.

The current implementation supports local lung segmentation through **lungmask R231**.

<p align="center"><img src="doc/diagrams/local-ai.svg" alt="Local AI pipeline" width="620"/></p>

The worker runs locally.

Medical images are not required to leave the host for inference.

On Apple Silicon, the local pipeline can automatically use **MPS** where available.

The AI subsystem is deliberately designed as a worker boundary rather than embedding a specific model directly into the PACS core. This allows future models and inference engines to be introduced without redesigning the storage or networking layers.

License note: the bundled `lungmask` plugin inherits the upstream **GPL-3.0** license, and `thorax-vessels` uses Apache-2.0 code with research-only weights. See `apps/viewer/ai-plugins/README.md`.

---

# Security & Access Control

AETHERIS provides application-level security mechanisms for distributed deployments:

* Argon2 password hashing
* JWT authentication
* Refresh tokens
* Role-Based Access Control
* Account management
* Approval-based password reset
* Audit logging
* Permission-aware API access
* Institution-scoped device and workflow authorization
* Independent report review permissions
* Versioned report amendments
* Lifecycle controls

The server owns database connections.

Clients never connect directly to PostgreSQL.

<p align="center"><img src="doc/diagrams/security-boundary.svg" alt="Security boundary" width="620"/></p>

This prevents database credentials from being distributed to every client and creates a clean security boundary between the application and persistence layers.

---

# Architecture

AETHERIS is organized as a Rust workspace with explicit subsystem boundaries.

<p align="center"><img src="doc/diagrams/repo-structure.svg" alt="Repository structure" width="620"/></p>

The architecture intentionally separates:

<p align="center"><img src="doc/diagrams/architecture-layers.svg" alt="Architecture layers" width="620"/></p>

This makes individual subsystems independently testable and replaceable.

---

# Technology Stack

| Layer            | Technology                |
| ---------------- | ------------------------- |
| Core language    | Rust                      |
| Desktop          | Tauri 2                   |
| Backend HTTP     | Axum                      |
| Database         | PostgreSQL                |
| DICOM            | DIMSE + DICOMweb          |
| Authentication   | Argon2 + JWT              |
| AI               | Local Worker Architecture |
| Containerization | Docker / Compose          |
| macOS            | Apple Silicon             |
| Windows          | x64                       |
| License          | MIT                       |

---

# Deployment

AETHERIS is designed to be deployable without requiring a complex infrastructure stack.

## Docker

```bash
docker compose up -d --build
```

The default development stack provides:

<p align="center"><img src="doc/diagrams/deployment-stack.svg" alt="Docker deployment stack" width="620"/></p>

The Tauri Viewer remains a native host application.

---

# DICOM Device Simulation

AETHERIS includes a DCMTK-based device simulator for development and interoperability testing.

```bash
python3 tools/dcmtk-simulator.py
```

The simulator can:

* Upload DICOM folders
* Configure Calling AE
* Configure Called AE
* Simulate multiple devices
* Perform concurrent transfers

This makes it possible to develop and test PACS networking without requiring physical CT, MR, CR, DR or other modalities.

---

# Zero-Dependency Desktop Distribution

AETHERIS can also be packaged as a standalone desktop application.

Current v0.3.0 packages:

* [macOS Apple Silicon DMG](https://github.com/LQ-1123/AETHERIS/releases/download/v0.3.0/AETHERIS_0.3.0_aarch64.dmg)
* [Windows 10/11 x64 installer](https://github.com/LQ-1123/AETHERIS/releases/download/v0.3.0/AETHERIS-Setup-0.3.0-x64.exe)

### macOS

<p align="center"><img src="doc/diagrams/packaging.svg" alt="macOS app bundle" width="620"/></p>

The resulting DMG is designed for an out-of-the-box local installation.

### Windows

GitHub Actions builds a Windows installer containing:

<p align="center"><img src="doc/diagrams/packaging.svg" alt="Windows installer contents" width="620"/></p>

The target machine does not need a separate PACS installation.

The v0.3.0 packages are not commercially code-signed. The macOS build uses ad-hoc signing and is not notarized; the Windows installer requires administrator privileges. Review the [release notes](doc/releases/v0.3.0.md) before installation or upgrade.

---

# Development

## Requirements

* Rust 1.97.1+
* PostgreSQL
* DCMTK
* Node.js
* npm
* Docker (optional)

## Build

```bash
cargo build --workspace
```

## Test

```bash
cargo test --workspace
```

## Lint

```bash
cargo clippy --workspace --all-targets -- -D warnings
```

## Run Viewer

```bash
cd apps/viewer

npm install
npm run tauri dev
```

---

# Interoperability Testing

AETHERIS does not rely exclusively on self-generated test traffic.

The project uses DCMTK to exercise real DICOM associations against the server.

For example:

```bash
echoscu \
  -aet TEST_SCU \
  -aec REMOTE_PACS \
  127.0.0.1 11112
```

And:

```bash
storescu \
  -aet TEST_SCU \
  -aec REMOTE_PACS \
  127.0.0.1 11112 \
  x.dcm
```

This allows the DIMSE implementation to be validated against an independent DICOM implementation.

---

# API Inspection

AETHERIS includes an API inspection center for development and integration testing.

After starting `pacsd`, open:

```text
https://127.0.0.1:8443/api-checker
```

The checker can inspect:

* OpenAPI routes
* DICOMweb endpoints
* Viewer APIs
* Annotation APIs
* Segmentation APIs
* Transfer APIs
* Authentication protection
* GET smoke tests
* JSON test exports

Write operations are never automatically executed during batch inspection.

---

# Engineering Principles

AETHERIS is built around several non-negotiable principles.

### 1. Durability before acknowledgement

If C-STORE returns success, the data must actually be durable.

### 2. Never guess medical image geometry

CT/MR ordering must be based on spatial metadata, not filenames.

### 3. Preserve original DICOM bytes

Storage should not introduce unnecessary lossy transformations.

### 4. Clients never own database credentials

The application server remains the security and authorization boundary.

### 5. Standards over vendor lock-in

DICOM and DICOMweb should remain the primary interoperability layer.

### 6. Local-first AI

AI inference should be capable of operating without sending medical images to a third-party cloud service.

### 7. Explicit failure over silent corruption

When the system cannot determine something safely, it should fail visibly rather than silently produce a plausible but incorrect result.

---

# Project Status

AETHERIS is under active development.

Current milestones:

```text
Phase 0 ──────────────────────────────── ✅
Core architecture

Phase 1 ──────────────────────────────── ✅
Storage / database

Phase 2 ──────────────────────────────── ✅
DIMSE infrastructure

Phase 3 ──────────────────────────────── ✅
Authentication / RBAC / audit

Phase 4 ──────────────────────────────── ✅
PACS workflows

Phase 5 ──────────────────────────────── 🟡
DICOMweb
QIDO-RS / WADO-RS      ✅
STOW-RS                ✅

Phase 6 ──────────────────────────────── 🟡
Native Viewer
Local DICOM            ✅
Remote Worklist        ✅
2D Visualization       ✅
3D Visualization       ✅
Local AI               ✅
```

---

# Roadmap

The long-term direction of AETHERIS includes:

* [ ] STOW-RS: application/dicom+json and bulk-data variants
* [ ] Expand DICOMweb coverage
* [ ] Improve DICOM modality interoperability
* [ ] Advanced MPR / VR workflows
* [ ] More AI segmentation models
* [ ] AI-assisted image analysis
* [x] Structured reporting (report workbench: findings/impression/recommendation, positive flag, sign/amend, version snapshots)
* [x] Report review workflow (peer review, reviewer-direct correction)
* [x] Exam request workflow (technician order entry)
* [x] Workload reporting (per-user statistics)
* [ ] DICOM SR integration
* [x] Advanced worklist management
* [ ] Distributed storage
* [ ] Object storage backends
* [ ] Improved multi-site deployment
* [ ] Production-grade certificate management
* [ ] More comprehensive observability
* [ ] Expanded automated interoperability testing

The objective is to evolve AETHERIS from a self-hosted PACS into a broader **medical imaging infrastructure platform**.

---

# Security Notice

DIMSE itself does not provide strong authentication. AE Titles can be spoofed.

For development, AETHERIS binds services to loopback by default.

Before deploying with real devices or real patient data, production deployments must provide appropriate:

* TLS certificates and SAN configuration
* Network segmentation
* Firewall rules
* Device allowlists
* Credential management
* Backup strategy
* Access auditing
* Data retention policies
* Privacy and regulatory controls

Real patient data may be subject to applicable regulations, including PIPL, GDPR, HIPAA, and local healthcare regulations.

**Do not expose the development configuration directly to the public Internet.**

---

# Clinical Disclaimer

AETHERIS is a research and engineering project.

It has **not been clinically validated** and is not a medical device.

Nothing in this repository should be interpreted as:

* a medical diagnosis;
* a clinical recommendation;
* a validated radiological workflow;
* a substitute for qualified medical professionals.

AI outputs are experimental and must not be used as the sole basis for clinical decisions.

---

# License

AETHERIS is released under the [MIT License](./LICENSE).

---

<p align="center">

**AETHERIS**

*Medical Imaging Infrastructure, Reimagined.*

Built with Rust · DICOM · Tauri · PostgreSQL

</p>
