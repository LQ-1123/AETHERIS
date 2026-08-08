#!/bin/sh
set -eu

SCRIPT_DIR=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
if command -v python3.11 >/dev/null 2>&1; then
  DEFAULT_PYTHON=python3.11
else
  DEFAULT_PYTHON=python3
fi
PYTHON_BIN=${PACS_AI_SETUP_PYTHON:-$DEFAULT_PYTHON}

"$PYTHON_BIN" -m venv "$SCRIPT_DIR/.venv"
"$SCRIPT_DIR/.venv/bin/python" -m pip install --upgrade pip
"$SCRIPT_DIR/.venv/bin/python" -m pip install -r "$SCRIPT_DIR/requirements.txt"
"$SCRIPT_DIR/.venv/bin/python" "$SCRIPT_DIR/worker.py" --models
