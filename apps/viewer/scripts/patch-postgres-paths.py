#!/usr/bin/env python3
"""把 postgres 二进制里编译内嵌的绝对路径改成相对路径（目标机没有 /opt/homebrew）。
带结尾 NUL 匹配：只替换独立完整的路径字符串，避免命中 "/opt/homebrew/lib/postgresql@14/libpq.5.dylib" 前缀。
用法: patch-postgres-paths.py <binary>"""
import pathlib
import sys

p = pathlib.Path(sys.argv[1])
data = bytearray(p.read_bytes())
# 运行时解析基准因进程而异（initdb 按 cwd、postgres 服务按二进制目录），
# 统一用 ../lib 与 ../share，由启动器在 pgdata 平级放 lib/share 软链兜住两种解析。
pairs = [
    (b"/opt/homebrew/lib/postgresql@14\x00", b"../lib/postgresql@14\x00"),
    (b"/opt/homebrew/share/postgresql@14\x00", b"../share/postgresql@14\x00"),
]
ok = False
for old, new in pairs:
    i = data.find(old)
    if i != -1:
        data[i : i + len(old)] = new + b"\x00" * (len(old) - len(new))
        ok = True
if ok:
    p.write_bytes(bytes(data))
    print(f"    路径补丁: {p.name}")
