# LungMask AI plugin

The desktop viewer runs this worker as a local child process. DICOM files are
read from their existing local paths and are never sent to the PACS server or
an external inference API.

## Setup

From `apps/viewer`, run:

```sh
./ai-plugins/lungmask/setup.sh
```

The plugin exposes two models: the 119 MB `R231` left/right lung model and the
combined `LTRCLobes` + `R231` five-lobe model. Weights download on first use.
On Apple Silicon, PyTorch uses MPS automatically when available. The lobe model
uses a smaller batch size to keep its expected peak below 5 GB on a 16 GB Mac.

Use `PACS_AI_PYTHON` to select another Python environment. The legacy
`PACS_AI_WORKER` override remains available for protocol-compatible workers.

## Protocol

`worker.py --models` prints the versioned model catalog as one JSON line.
Inference uses a temporary request and output file:

```sh
python worker.py --request request.json --output result.json
```

Progress and sanitized errors are written as JSON lines to stdout. Results use
the viewer's `rle-v1` binary mask encoding. Temporary manifests are deleted by
the Rust adapter after each task.
