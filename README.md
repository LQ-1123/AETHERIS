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
![DICOM](https://img.shields.io/badge/DICOM-DIMSE%20%7C%20DICOMweb-0B6E99)
![License](https://img.shields.io/badge/License-MIT-green)
![Platform](https://img.shields.io/badge/Platform-Windows%20%7C%20macOS-lightgrey)

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
* **Worklists and report lifecycle management**
* **Docker-based deployment**
* **Windows and macOS distributable applications**

AETHERIS is intended to be both a usable PACS platform and an engineering foundation for future intelligent medical imaging workflows.

> **Research / engineering project. Not clinically validated. Not intended for diagnosis or direct clinical decision-making.**

---

## Screenshots

| Login | Worklist | 2D Viewer |
|---|---|---|
| ![Login](doc/screenshots/login.png) | ![Worklist](doc/screenshots/worklist.png) | ![Viewer](doc/screenshots/viewer.png) |

| MPR | Volume Rendering | AI Segmentation |
|---|---|---|
| ![MPR](doc/screenshots/mpr.png) | ![Volume Rendering](doc/screenshots/volume-rendering.png) | ![AI Segmentation](doc/screenshots/ai-segmentation.png) |

| Annotations | DICOM Tag Revision | Lifecycle |
|---|---|---|
| ![Annotations](doc/screenshots/annotations.png) | ![Tag Revision](doc/screenshots/tag-revision.png) | ![Lifecycle](doc/screenshots/lifecycle.png) |

| DICOM Router |
|---|
| ![Router](doc/screenshots/router.png) |

---

## Why AETHERIS?

Traditional PACS deployments often involve a collection of tightly coupled systems, vendor-specific configurations, and infrastructure that is difficult to reproduce outside a hospital environment.

AETHERIS takes a different approach:

```text
                    ┌─────────────────────────────┐
                    │          AETHERIS           │
                    │  Medical Imaging Platform   │
                    └──────────────┬──────────────┘
                                   │
            ┌──────────────────────┼──────────────────────┐
            │                      │                      │
            ▼                      ▼                      ▼
     DICOM / DIMSE            DICOMweb              Native Viewer
     C-ECHO / C-STORE         QIDO / WADO            Tauri 2
     C-FIND / C-MOVE          STOW*                  2D / 3D
     C-GET                                              MPR / VR
            │                      │                      │
            └──────────────────────┼──────────────────────┘
                                   │
                                   ▼
                         ┌──────────────────┐
                         │   Rust Core      │
                         │                  │
                         │ Storage / DB     │
                         │ Auth / Codec     │
                         │ AI / Workflows   │
                         └────────┬─────────┘
                                  │
                   ┌──────────────┴──────────────┐
                   ▼                             ▼
             PostgreSQL                  Byte-Fidelity Archive
             Metadata                    Durable DICOM Storage
```

The goal is not simply to make another DICOM viewer.

The goal is to build a **complete, composable medical imaging infrastructure**.

---

# Core Capabilities

## DICOM Networking

AETHERIS implements the core DIMSE services required for PACS interoperability:

| Service     | Status |
| ----------- | ------ |
| C-ECHO SCP  | ✅      |
| C-STORE SCP | ✅      |
| C-FIND SCP  | ✅      |
| C-MOVE SCP  | ✅      |
| C-GET SCP   | ✅      |

The DIMSE layer is implemented in the Rust workspace rather than relying entirely on a third-party PACS server.

This makes the protocol layer explicit, testable, and extensible.

---

## DICOMweb

Modern HTTP-based interoperability is provided through DICOMweb:

| Standard | Status            |
| -------- | ----------------- |
| QIDO-RS  | ✅                 |
| WADO-RS  | ✅                 |
| STOW-RS  | 🚧 In development |

DICOMweb provides a clean bridge between traditional modality infrastructure and modern web-based applications.

---

## Durable Storage

AETHERIS treats image persistence as a correctness problem rather than simply a file-copy operation.

The C-STORE path follows:

```text
DICOM Receive
     │
     ▼
Parse & Validate
     │
     ▼
Temporary File
     │
     ▼
fsync
     │
     ▼
Atomic Rename
     │
     ▼
fsync Parent Directory
     │
     ▼
PostgreSQL Transaction
     │
     ▼
C-STORE Success
```

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

### Geometry-aware Series Reconstruction

Series ordering does not rely on filenames or `InstanceNumber`.

AETHERIS uses:

```text
ImagePositionPatient
          +
ImageOrientationPatient
          ↓
Slice Geometry
          ↓
Slice Normal
          ↓
Spatial Ordering
```

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

The architecture is intended to support progressively more advanced volumetric visualization without coupling the viewer to the server implementation.

---

# Local AI

AETHERIS includes a local AI worker architecture for medical image processing.

The current implementation supports local lung segmentation through **lungmask R231**.

```text
                  DICOM
                    │
                    ▼
              AETHERIS Viewer
                    │
                    ▼
              Local AI Worker
                    │
              ┌─────┴─────┐
              │           │
              ▼           ▼
           Inference    Validation
              │           │
              └─────┬─────┘
                    ▼
                3D Mask
                    │
                    ▼
            Viewer Visualization
```

The worker runs locally.

Medical images are not required to leave the host for inference.

On Apple Silicon, the local pipeline can automatically use **MPS** where available.

The AI subsystem is deliberately designed as a worker boundary rather than embedding a specific model directly into the PACS core. This allows future models and inference engines to be introduced without redesigning the storage or networking layers.

---

# Security & Access Control

AETHERIS provides application-level security mechanisms for distributed deployments:

* Argon2 password hashing
* JWT authentication
* Refresh tokens
* Role-Based Access Control
* Account management
* Audit logging
* Permission-aware API access
* Versioned report amendments
* Lifecycle controls

The server owns database connections.

Clients never connect directly to PostgreSQL.

```text
             Viewer
                │
                │ HTTPS
                ▼
        ┌───────────────┐
        │    pacsd      │
        │               │
        │ Auth / RBAC   │
        │ DICOMweb      │
        │ Workflows     │
        └───────┬───────┘
                │
                │ Internal DB access
                ▼
           PostgreSQL
```

This prevents database credentials from being distributed to every client and creates a clean security boundary between the application and persistence layers.

---

# Architecture

AETHERIS is organized as a Rust workspace with explicit subsystem boundaries.

```text
AETHERIS/
│
├── crates/
│   ├── pacs-core/       Domain model, UID validation, DICOM metadata
│   ├── pacs-store/      Durable file storage and sharding
│   ├── pacs-db/         PostgreSQL access and migrations
│   ├── pacs-dimse/      DIMSE networking
│   ├── pacs-auth/       Authentication, RBAC and audit
│   ├── pacs-web/        Axum + DICOMweb + REST APIs
│   ├── pacs-codec/      Pixel decoding and frame extraction
│   ├── pacs-ai/         Local AI worker protocol
│   └── pacsd/           Server entrypoint
│
├── apps/
│   └── viewer/          Tauri 2 desktop application
│
├── docker/
│   └── ...              Container deployment resources
│
├── tools/
│   └── ...              DICOM tooling and simulators
│
└── .github/
    └── workflows/       CI / release automation
```

The architecture intentionally separates:

```text
Protocol
   ↓
Domain
   ↓
Persistence
   ↓
Application Services
   ↓
API
   ↓
Desktop Client
```

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

```text
PostgreSQL
     +
pacsd
     +
DICOM Device Simulator
```

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

### macOS

```text
AETHERIS.app
    │
    ├── Tauri Viewer
    ├── pacsd
    ├── PostgreSQL
    └── bundled dependencies
```

The resulting DMG is designed for an out-of-the-box local installation.

### Windows

GitHub Actions builds a Windows installer containing:

```text
AETHERIS
├── Viewer
├── pacsd
├── PostgreSQL
├── Launcher
└── Runtime dependencies
```

The target machine does not need a separate PACS installation.

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
STOW-RS                🚧

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

* [ ] Complete STOW-RS
* [ ] Expand DICOMweb coverage
* [ ] Improve DICOM modality interoperability
* [ ] Advanced MPR / VR workflows
* [ ] More AI segmentation models
* [ ] AI-assisted image analysis
* [ ] Structured reporting
* [ ] DICOM SR integration
* [ ] Advanced worklist management
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
