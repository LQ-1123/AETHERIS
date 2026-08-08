#!/usr/bin/env python3
"""Bundled LungMask plugin implementing the AETHERIS Worker v1 protocol."""

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
LUNG_LABELS = (
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
LOBE_LABELS = (
    {
        "id": "left-upper-lobe",
        "display_name": "左肺上叶",
        "color": [236, 112, 99],
        "tags": ["AI", "肺叶", "左肺上叶"],
    },
    {
        "id": "left-lower-lobe",
        "display_name": "左肺下叶",
        "color": [245, 176, 65],
        "tags": ["AI", "肺叶", "左肺下叶"],
    },
    {
        "id": "right-upper-lobe",
        "display_name": "右肺上叶",
        "color": [88, 214, 141],
        "tags": ["AI", "肺叶", "右肺上叶"],
    },
    {
        "id": "right-middle-lobe",
        "display_name": "右肺中叶",
        "color": [93, 173, 226],
        "tags": ["AI", "肺叶", "右肺中叶"],
    },
    {
        "id": "right-lower-lobe",
        "display_name": "右肺下叶",
        "color": [165, 105, 189],
        "tags": ["AI", "肺叶", "右肺下叶"],
    },
)

MODEL_SPECS = {
    "lungmask-r231": {
        "display_name": "左右肺分割 R231",
        "description": "轻量 CT 左右肺分割",
        "labels": LUNG_LABELS,
        "modelname": "R231",
        "fillmodel": None,
        "batch_size": 4,
        "memory_mb": 3600,
        "download_mb": 119,
    },
    "lungmask-lobes-r231": {
        "display_name": "五肺叶分割 LTRCLobes",
        "description": "CT 左上叶、左下叶、右上叶、右中叶和右下叶分割",
        "labels": LOBE_LABELS,
        "modelname": "LTRCLobes",
        "fillmodel": "R231",
        "batch_size": 2,
        "memory_mb": 4600,
        "download_mb": 238,
    },
}


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
    models = []
    for model_id, spec in MODEL_SPECS.items():
        models.append(
            {
                "id": model_id,
                "display_name": spec["display_name"],
                "version": version,
                "description": spec["description"],
                "supported_modalities": ["CT"],
                "labels": list(spec["labels"]),
                "estimated_peak_memory_mb": spec["memory_mb"],
                "model_download_mb": spec["download_mb"],
                "device": device,
                "available": available,
                "unavailable_reason": reason,
            }
        )
    return {"protocol_version": PROTOCOL_VERSION, "models": models}


def parse_request(path: Path) -> dict[str, Any]:
    try:
        request = json.loads(path.read_text(encoding="utf-8"))
    except (OSError, json.JSONDecodeError) as error:
        raise WorkerFailure("无法读取 AI 请求") from error
    if request.get("protocol_version") != PROTOCOL_VERSION:
        raise WorkerFailure("AI Worker 协议版本不兼容")
    if request.get("model_id") not in MODEL_SPECS:
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
    spec = MODEL_SPECS[request["model_id"]]
    emit_progress(job_id, "loading", 1, 4, "正在读取本地 CT 体数据")
    image = read_volume(request)
    emit_progress(job_id, "model", 2, 4, "正在加载肺部分割模型（首次运行会下载权重）")
    try:
        inferer = LMInferer(
            modelname=spec["modelname"],
            fillmodel=spec["fillmodel"],
            batch_size=spec["batch_size"],
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
    for label_value, descriptor in enumerate(spec["labels"], start=1):
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
