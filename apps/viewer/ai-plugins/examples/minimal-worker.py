#!/usr/bin/env python3
"""Minimal Worker v1 adapter skeleton; replace run_model with real inference."""

from __future__ import annotations

import argparse
import json
from pathlib import Path


def emit(value: dict) -> None:
    print(json.dumps(value, separators=(",", ":")), flush=True)


def catalog() -> dict:
    return {
        "protocol_version": 1,
        "models": [
            {
                "id": "example",
                "display_name": "Example",
                "version": "1",
                "description": "Replace with a real local model",
                "supported_modalities": ["CT"],
                "labels": [
                    {
                        "id": "target",
                        "display_name": "Target",
                        "color": [55, 213, 216],
                        "tags": ["AI"],
                    }
                ],
                "estimated_peak_memory_mb": 1024,
                "model_download_mb": 0,
                "device": "CPU",
                "available": True,
                "unavailable_reason": None,
            }
        ],
    }


def run_model(request_path: Path, output_path: Path) -> None:
    request = json.loads(request_path.read_text(encoding="utf-8"))
    if request.get("protocol_version") != 1:
        raise RuntimeError("unsupported protocol")
    emit(
        {
            "type": "progress",
            "job_id": request["job_id"],
            "stage": "inference",
            "completed": 0,
            "total": 1,
            "message": "Replace run_model with local inference",
        }
    )
    raise RuntimeError("example adapter does not implement inference")


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--models", action="store_true")
    parser.add_argument("--request", type=Path)
    parser.add_argument("--output", type=Path)
    args = parser.parse_args()
    if args.models:
        emit(catalog())
        return 0
    try:
        run_model(args.request, args.output)
    except Exception as error:
        emit({"type": "error", "message": str(error)})
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
