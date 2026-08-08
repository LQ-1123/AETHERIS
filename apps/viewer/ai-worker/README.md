# Local AI worker

The desktop viewer runs this worker as a local child process. DICOM files are
read from their existing local paths and are never sent to the PACS server or
an external inference API.

## Setup

From `apps/viewer`, run:

```sh
./ai-worker/setup.sh
```

The default `lungmask R231` weights are about 119 MB and download on the first
inference. On Apple Silicon, PyTorch uses MPS automatically when available.
The tested peak memory for a 393-slice 512 x 512 CT volume is about 3.4 GB.

Use `PACS_AI_PYTHON` to select another Python environment and
`PACS_AI_WORKER` to select a protocol-compatible worker script.

## Protocol

`worker.py --models` prints the versioned model catalog as one JSON line.
Inference uses a temporary request and output file:

```sh
python worker.py --request request.json --output result.json
```

Progress and sanitized errors are written as JSON lines to stdout. Results use
the viewer's `rle-v1` binary mask encoding. Temporary manifests are deleted by
the Rust adapter after each task.
