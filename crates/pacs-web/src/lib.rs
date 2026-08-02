//! DICOMweb 与认证 HTTP API(axum)。
//!
//! QIDO-RS(复用 pacs-db 查询层)、WADO-RS(含 `/frames`,Range 支持)、
//! STOW-RS。全部端点带认证;帧级 LRU 缓存。
