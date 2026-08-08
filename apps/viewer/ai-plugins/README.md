# AETHERIS local AI plugins

AI plugins are trusted local programs. A plugin receives paths to local DICOM
instances and must implement Worker protocol version 1. The viewer does not
provide an HTTP inference transport and never uploads images on a plugin's
behalf.

## Install a user plugin

Create one subdirectory per plugin in the application data directory:

- macOS: `~/Library/Application Support/cn.local.remote-pacs.viewer/ai-plugins`
- Windows: `%APPDATA%/cn.local.remote-pacs.viewer/ai-plugins`
- Linux: `$XDG_DATA_HOME/cn.local.remote-pacs.viewer/ai-plugins`

Each subdirectory needs a `plugin.json` matching `plugin.schema.json`. Python
plugins should keep dependencies in a sibling `.venv`; executable plugins must
place their entrypoint inside the plugin directory. Restart the viewer or use
the refresh button in the Mask menu after changing plugins.

Installed plugins are local code with the same file access as the viewer. Only
install plugins from a trusted source. Version 1 validates paths and process
output but does not provide code signing or an operating-system network sandbox.

## Worker protocol v1

`worker.py --models` prints a final JSON line containing:

```json
{"protocol_version":1,"models":[{"id":"example","display_name":"Example","version":"1","description":"Example binary segmentation","supported_modalities":["CT"],"labels":[{"id":"target","display_name":"Target","color":[55,213,216],"tags":["AI"]}],"estimated_peak_memory_mb":1024,"model_download_mb":0,"device":"CPU","available":true,"unavailable_reason":null}]}
```

Inference is invoked with `--request request.json --output result.json`. The
request contains the job ID, raw model ID, rows, columns, modality, and ordered
`slices` with `source_index` and DICOM `path`. Inputs are read-only. Output must
contain one binary `rle-v1` mask for every source slice and label. Runs are
little-endian unsigned 32-bit counts, alternating background and foreground;
use an initial zero run when the first pixel is foreground.

Progress is one JSON object per stdout line:

```json
{"type":"progress","job_id":"...","stage":"inference","completed":2,"total":3,"message":"Running model"}
```

Sanitized failures may be emitted as `{"type":"error","message":"..."}`
before exiting nonzero. See `examples/minimal-worker.py` for an adapter skeleton.
