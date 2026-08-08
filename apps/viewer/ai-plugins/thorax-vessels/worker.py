#!/usr/bin/env python3
"""Focused TotalSegmentator adapter for pulmonary vessels and central airways."""

from __future__ import annotations

import argparse
import base64
import importlib.metadata
import json
import os
from pathlib import Path
import sys
import tempfile
import threading
import time
from typing import Any


PROTOCOL_VERSION = 1
MODEL_ID = "totalsegmentator-lung-vessels"
for variable in ("OMP_NUM_THREADS", "MKL_NUM_THREADS", "ITK_GLOBAL_DEFAULT_NUMBER_OF_THREADS"):
    os.environ.setdefault(variable, "4")
LABELS = (
    {
        "id": "pulmonary-vessels",
        "display_name": "肺血管",
        "color": [224, 72, 84],
        "tags": ["AI", "肺", "血管", "肺血管"],
        "filename": "lung_vessels.nii.gz",
    },
    {
        "id": "trachea-bronchi",
        "display_name": "气管与支气管",
        "color": [76, 198, 210],
        "tags": ["AI", "肺", "气道", "气管", "支气管"],
        "filename": "lung_trachea_bronchia.nii.gz",
    },
)


class WorkerFailure(Exception):
    pass


def emit(payload: dict[str, Any]) -> None:
    print(json.dumps(payload, ensure_ascii=False, separators=(",", ":")), flush=True)


def selected_device() -> tuple[str, str]:
    requested = os.environ.get("AETHERIS_TOTAL_DEVICE", "cpu").strip().lower()
    if requested == "mps":
        try:
            import torch

            if getattr(torch.backends, "mps", None) is not None and torch.backends.mps.is_available():
                return "mps", "Apple MPS"
        except (ImportError, OSError):
            pass
    return "cpu", "CPU（稳定模式）"


def runtime_status() -> tuple[bool, str, str | None, str]:
    _, device_name = selected_device()
    try:
        import SimpleITK  # noqa: F401
        import totalsegmentator  # noqa: F401

        version = importlib.metadata.version("TotalSegmentator")
    except (ImportError, OSError, importlib.metadata.PackageNotFoundError):
        return False, device_name, "可选依赖尚未安装，请运行 thorax-vessels/setup.sh", "2.x"
    return True, device_name, None, version


def model_catalog() -> dict[str, Any]:
    available, device, reason, version = runtime_status()
    labels = [{key: value for key, value in label.items() if key != "filename"} for label in LABELS]
    return {
        "protocol_version": PROTOCOL_VERSION,
        "models": [
            {
                "id": MODEL_ID,
                "display_name": "肺血管与气道分割",
                "version": version,
                "description": "聚焦胸部 CT 的肺血管、气管和支气管分割",
                "supported_modalities": ["CT"],
                "labels": labels,
                "estimated_peak_memory_mb": 8192,
                "model_download_mb": 350,
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
        raise WorkerFailure("肺血管模型仅支持 CT 序列")
    return request


def read_volume(request: dict[str, Any]):
    try:
        import SimpleITK as sitk
    except (ImportError, OSError) as error:
        raise WorkerFailure("可选 AI 依赖尚未安装") from error

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
    return base64.b64encode(np.asarray(runs, dtype="<u4").tobytes()).decode("ascii")


def progress(job_id: str, stage: str, completed: int, total: int, message: str) -> None:
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


def inference_heartbeat(
    job_id: str,
    stopped: threading.Event,
    started_at: float,
    device_name: str,
) -> None:
    while not stopped.wait(15):
        elapsed_minutes = max(1, round((time.monotonic() - started_at) / 60))
        progress(
            job_id,
            "inference",
            2,
            4,
            f"肺血管模型仍在 {device_name} 推理，已运行约 {elapsed_minutes} 分钟",
        )


def run_segmentation(request: dict[str, Any], output_path: Path) -> None:
    try:
        import numpy as np
        import SimpleITK as sitk
        from totalsegmentator.python_api import totalsegmentator
    except (ImportError, OSError) as error:
        raise WorkerFailure("可选 AI 依赖尚未安装，请运行 thorax-vessels/setup.sh") from error

    job_id = request["job_id"]
    progress(job_id, "loading", 1, 4, "正在读取本地胸部 CT")
    image = read_volume(request)
    with tempfile.TemporaryDirectory(prefix="aetheris-thorax-") as temporary:
        root = Path(temporary)
        input_path = root / "input.nii.gz"
        output_dir = root / "segments"
        try:
            sitk.WriteImage(image, os.fspath(input_path), True)
        except RuntimeError as error:
            raise WorkerFailure("无法准备 AI 体数据") from error

        device, device_name = selected_device()
        progress(job_id, "model", 2, 4, "正在加载肺血管模型（首次运行会下载权重）")
        heartbeat_stopped = threading.Event()
        inference_started = time.monotonic()
        heartbeat = threading.Thread(
            target=inference_heartbeat,
            args=(job_id, heartbeat_stopped, inference_started, device_name),
            daemon=True,
        )
        heartbeat.start()
        try:
            totalsegmentator(
                input=input_path,
                output=output_dir,
                task="lung_vessels",
                device=device,
                nr_thr_resamp=1,
                nr_thr_saving=1,
                quiet=True,
                verbose=False,
            )
        except Exception as error:
            raise WorkerFailure("肺血管 AI 推理失败，请检查模型权重、内存和网络") from error
        finally:
            heartbeat_stopped.set()
            heartbeat.join(timeout=1)

        progress(job_id, "inference", 3, 4, "正在读取肺血管与气道结果")
        series = request["series"]
        expected_shape = (len(series["slices"]), int(series["rows"]), int(series["cols"]))
        segments = []
        for descriptor in LABELS:
            mask_path = output_dir / descriptor["filename"]
            if not mask_path.is_file():
                raise WorkerFailure(f"AI 未生成预期结果: {descriptor['display_name']}")
            try:
                binary = sitk.GetArrayFromImage(sitk.ReadImage(os.fspath(mask_path))) > 0
            except RuntimeError as error:
                raise WorkerFailure("无法读取 AI Mask") from error
            if tuple(binary.shape) != expected_shape:
                raise WorkerFailure("AI 输出尺寸与输入序列不一致")
            label = {key: value for key, value in descriptor.items() if key != "filename"}
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
                    "label": label,
                    "voxel_count": int(np.count_nonzero(binary)),
                    "masks": masks,
                }
            )

    progress(job_id, "encoding", 4, 4, "正在生成可编辑 Mask")
    result = {
        "protocol_version": PROTOCOL_VERSION,
        "job_id": job_id,
        "model_id": request["model_id"],
        "elapsed_ms": int((time.monotonic() - STARTED_AT) * 1000),
        "segments": segments,
    }
    temporary_output = output_path.with_suffix(output_path.suffix + ".tmp")
    try:
        temporary_output.write_text(
            json.dumps(result, ensure_ascii=False, separators=(",", ":")),
            encoding="utf-8",
        )
        os.replace(temporary_output, output_path)
    except OSError as error:
        raise WorkerFailure("无法写入 AI 分割结果") from error


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
        run_segmentation(parse_request(args.request), args.output)
        return 0
    except WorkerFailure as error:
        emit({"type": "error", "message": str(error)})
        return 1


STARTED_AT = time.monotonic()


if __name__ == "__main__":
    sys.exit(main())
