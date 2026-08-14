#!/usr/bin/env bash
# pacsd 容器入口：等待数据库 → 可选初始化管理员 → 启动服务端。
# 数据库迁移由 pacsd 启动时自动应用，无需手动执行。
set -euo pipefail

DB_HOST="${POSTGRES_HOST:-postgres}"
DB_PORT="${POSTGRES_PORT:-5432}"

echo "[entrypoint] 等待数据库 ${DB_HOST}:${DB_PORT} 就绪 ..."
ready=0
for _ in $(seq 1 90); do
  if (exec 3<>"/dev/tcp/${DB_HOST}/${DB_PORT}") 2>/dev/null; then
    exec 3>&- 2>/dev/null || true
    ready=1
    break
  fi
  sleep 1
done
if [[ "$ready" != "1" ]]; then
  echo "[entrypoint] 错误：数据库 ${DB_HOST}:${DB_PORT} 90 秒内未就绪" >&2
  exit 1
fi

if [[ -n "${PACS_ADMIN_USERNAME:-}" && -n "${PACS_ADMIN_PASSWORD:-}" ]]; then
  echo "[entrypoint] 初始化管理员账号 ${PACS_ADMIN_USERNAME} ..."
  if ! output="$(pacsd admin --username "${PACS_ADMIN_USERNAME}" --password "${PACS_ADMIN_PASSWORD}" 2>&1)"; then
    if [[ "$output" == *"已存在"* ]]; then
      echo "[entrypoint] 管理员已存在，跳过创建"
    else
      echo "[entrypoint] 管理员创建失败：" >&2
      echo "$output" >&2
      exit 1
    fi
  else
    echo "$output"
  fi
fi

echo "[entrypoint] 启动 pacsd ..."
exec pacsd "$@"
