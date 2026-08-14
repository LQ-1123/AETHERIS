# syntax=docker/dockerfile:1
#
# remote_pacs 服务端 pacsd 镜像。
# 数据库迁移已编译进二进制，运行时不需要 SQL 文件。
# 唯一的系统级 C 库是 libarchive（compress-tools 用于 ZIP/RAR 导入导出），
# 构建与运行阶段均已显式安装。

# ===== 构建阶段 =====
# rust-toolchain.toml 会把工具链固定到 1.97.1（镜像自带 rustup，联网自动补装）。
FROM rust:1.97-bookworm AS builder
WORKDIR /src

# compress-tools（ZIP/RAR 导入导出）通过 pkg-config 链接 libarchive，Linux 下必须显式安装。
RUN apt-get update \
    && apt-get install -y --no-install-recommends pkg-config libarchive-dev \
    && rm -rf /var/lib/apt/lists/*

# 拷贝全部源码；.dockerignore 已排除 target/、node_modules/、数据与打包物。
COPY . .

# 只构建 pacsd 及其依赖树。
# --mount=type=cache 复用 registry 与 target，增量构建只重编变化的 crate。
# 注意：cache mount 不写入镜像层，产物必须在本步内拷到 /out 才能被 COPY 带走。
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release -p pacsd \
    && mkdir -p /out \
    && cp /src/target/release/pacsd /out/pacsd

# ===== 运行阶段 =====
FROM debian:bookworm-slim

# libarchive13：pacsd 运行时动态链接 libarchive（ZIP/RAR 导入导出）
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates curl libarchive13 \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /out/pacsd /usr/local/bin/pacsd
COPY docker/entrypoint.sh /usr/local/bin/entrypoint.sh
RUN chmod +x /usr/local/bin/entrypoint.sh

# 容器内默认绑定 0.0.0.0（端口由 compose 映射到宿主机）；
# TLS 自签证书由 pacsd 首次启动时自动生成到 ${PACS_STORAGE_ROOT}/tls/。
ENV PACS_STORAGE_ROOT=/data \
    PACS_HTTP_BIND=0.0.0.0:8443 \
    PACS_DIMSE_BIND=0.0.0.0:11112 \
    PACS_AE_TITLE=REMOTE_PACS

EXPOSE 11112 8443
ENTRYPOINT ["/usr/local/bin/entrypoint.sh"]
