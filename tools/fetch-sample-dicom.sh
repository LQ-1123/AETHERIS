#!/usr/bin/env bash
# 下载公开示例 DICOM（pydicom 仓库的测试数据，无患者隐私），供模拟器上传演示。
# 用法：./tools/fetch-sample-dicom.sh [输出目录，默认 data/samples]
set -euo pipefail

DIR="${1:-data/samples}"
mkdir -p "$DIR"

base="https://raw.githubusercontent.com/pydicom/pydicom/master/src/pydicom/data/test_files"
files=(CT_small.dcm MR_small.dcm)

for f in "${files[@]}"; do
  if [[ -f "$DIR/$f" ]]; then
    echo "已存在 $DIR/$f，跳过"
    continue
  fi
  if curl -fsSL --retry 3 --connect-timeout 10 "$base/$f" -o "$DIR/$f"; then
    echo "✓ $DIR/$f"
  else
    echo "✗ 下载失败：$f（需要联网；也可以手动准备任意 .dcm 文件）"
  fi
done

echo
echo "下一步：打开 http://127.0.0.1:8787（DCMTK 模拟器），把上面的文件拖进去，"
echo "设备配置里“主机”填 pacsd、端口 11112，点“开始并发上传”。"
