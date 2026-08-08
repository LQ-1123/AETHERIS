#!/usr/bin/env python3
"""Local, versioned AI segmentation worker for the desktop viewer."""

from __future__ import annotations

import argparse
import base64
import importlib.metadata
import json
import os
from pathlib import Path
import sys
import time
from typing import Any


PROTOCOL_VERSION = 1
MODEL_ID = "lungmask-r231"
MODEL_LABELS = (
    {
        "id": "right-lung",
        "display_name": "右肺",
        "color": [52, 184, 224],
        "tags": ["AI", "肺", "右肺"],
    },
    {
        "id": "left-lung",
        "display_name": "左肺",
        "color": [242, 154, 67],
        "tags": ["AI", "肺", "左肺"],
    },
)


def emit(payload: dict[str, Any]) -> None:
    print(json.dumps(payload, ensure_ascii=False, separators=(",", ":")), flush=True)


def runtime_status() -> tuple[bool, str | None, str | None]:
    try:
        import SimpleITK  # noqa: F401
        import lungmask  # noqa: F401
        import torch
    except (ImportError, OSError):
        return False, None, "本地 AI 依赖尚未安装"

    if getattr(torch.backends, "mps", None) is not None and torch.backends.mps.is_available():
        device = "Apple MPS"
    elif torch.cuda.is_available():
        device = "CUDA"
    else:
        device = "CPU"
    return True, device, None


def model_catalog() -> dict[str, Any]:
    available, device, reason = runtime_status()
    try:
        version = importlib.metadata.version("lungmask") if available else "0.2.21"
    except importlib.metadata.PackageNotFoundError:
        version = "0.2.21"
    return {
        "protocol_version": PROTOCOL_VERSION,
        "models": [
            {
                "id": MODEL_ID,
                "display_name": "肺部分割 R231",
                "version": version,
                "description": "CT 左右肺分割",
                "supported_modalities": ["CT"],
                "labels": list(MODEL_LABELS),
                "estimated_peak_memory_mb": 3600,
                "model_download_mb": 119,
                "device": device,
                "available": available,
                "unavailable_reason": reason,
            }
        ],
    }


def parse_request(path: Path) -> dict[str, Any]:
    try:
        request = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise WorkerFailure("无法读取 AI 请求") from error
    if request.get("protocol_version") != PROTOCOL_VERSION:
        raise WorkerFailure("AI Worker 协议版本不兼容")
    if request.get("model_id") != MODEL_ID:
        raise WorkerFailure("请求的 AI 模型不可用")
    series = request.get("series")
    if not isinstance(series, dict) or len(series.get("slices", [])) < 2:
        raise WorkerFailure("AI 输入序列无效")
    if (series.get("modality") or "").upper() != "CT":
        raise WorkerFailure("肺部分割模型仅支持 CT 序列")
    return request


def read_volume(request: dict[str, Any]):
    try:
        import SimpleITK as sitk
    except (ImportError, OSError) as error:
        raise WorkerFailure("本地 AI 依赖尚未安装") from error

    series = request["series"]
    files = [Path(item["path"]) for item in series["slices"]]
    if any(not path.is_file() for path in files):
        raise WorkerFailure("AI 输入切片不存在")
    try:
        reader = sitk.ImageSeriesReader()
        reader.SetFileNames([os.fspath(path) for path in files])
        image = reader.Execute()
    except RuntimeError as error:
        raise WorkerFailure("无法读取 DICOM 体数据") from error

    cols, rows, slices = image.GetSize()
    if (rows, cols, slices) != (
        int(series["rows"]),
        int(series["cols"]),
        len(series["slices"]),
    ):
        raise WorkerFailure("DICOM 体数据尺寸与 Viewer 序列不一致")
    return image


def encode_rle(mask) -> str:
    import numpy as np

    flat = np.asarray(mask, dtype=np.uint8).reshape(-1)
    if flat.size == 0:
        raise WorkerFailure("AI Mask 为空")
    changes = np.flatnonzero(flat[1:] != flat[:-1]) + 1
    bounds = np.concatenate((np.array([0]), changes, np.array([flat.size])))
    runs = np.diff(bounds)
    if flat[0] != 0:
        runs = np.concatenate((np.array([0]), runs))
    encoded = np.asarray(runs, dtype="<u4").tobytes()
    return base64.b64encode(encoded).decode("ascii")


def run_segmentation(request: dict[str, Any], output_path: Path) -> None:
    try:
        import numpy as np
        from lungmask import LMInferer
    except (ImportError, OSError) as error:
        raise WorkerFailure("本地 AI 依赖尚未安装") from error

    job_id = request["job_id"]
    emit_progress(job_id, "loading", 1, 4, "正在读取本地 CT 体数据")
    image = read_volume(request)
    emit_progress(job_id, "model", 2, 4, "正在加载肺部分割模型")
    try:
        inferer = LMInferer(
            modelname="R231",
            batch_size=4,
            volume_postprocessing=True,
            tqdm_disable=True,
        )
    except Exception as error:
        raise WorkerFailure("无法加载肺部分割模型") from error

    emit_progress(job_id, "inference", 3, 4, "正在执行本地 AI 推理")
    try:
        labels = inferer.apply(image)
    except Exception as error:
        raise WorkerFailure("AI 推理失败") from error

    series = request["series"]
    expected_shape = (len(series["slices"]), int(series["rows"]), int(series["cols"]))
    if tuple(labels.shape) != expected_shape:
        raise WorkerFailure("AI 输出尺寸与输入序列不一致")

    segments = []
    for label_value, descriptor in enumerate(MODEL_LABELS, start=1):
        binary = labels == label_value
        masks = [
            {
                "source_index": int(source["source_index"]),
                "rows": int(series["rows"]),
                "cols": int(series["cols"]),
                "encoding": "rle-v1",
                "data_base64": encode_rle(binary[index]),
            }
            for index, source in enumerate(series["slices"])
        ]
        segments.append(
            {
                "label": descriptor,
                "voxel_count": int(np.count_nonzero(binary)),
                "masks": masks,
            }
        )

    result = {
        "protocol_version": PROTOCOL_VERSION,
        "job_id": job_id,
        "model_id": request["model_id"],
        "elapsed_ms": int((time.monotonic() - STARTED_AT) * 1000),
        "segments": segments,
    }
    emit_progress(job_id, "encoding", 4, 4, "正在生成可编辑 Mask")
    temporary = output_path.with_suffix(output_path.suffix + ".tmp")
    try:
        temporary.write_text(
            json.dumps(result, ensure_ascii=False, separators=(",", ":")),
            encoding="utf-8",
        )
        os.replace(temporary, output_path)
    except OSError as error:
        raise WorkerFailure("无法写入 AI 分割结果") from error


def emit_progress(job_id: str, stage: str, completed: int, total: int, message: str) -> None:
    emit(
        {
            "type": "progress",
            "job_id": job_id,
            "stage": stage,
            "completed": completed,
            "total": total,
            "message": message,
        }
    )


class WorkerFailure(Exception):
    pass


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--models", action="store_true")
    parser.add_argument("--request", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()

    if args.models:
        emit(model_catalog())
        return 0
    if args.request is None or args.output is None:
        parser.error("--request and --output are required")

    try:
        request = parse_request(args.request)
        run_segmentation(request, args.output)
        return 0
    except WorkerFailure as error:
        emit({"type": "error", "message": str(error)})
        return 1


STARTED_AT = time.monotonic()


if __name__ == "__main__":
    sys.exit(main())
