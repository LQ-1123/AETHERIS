# Thorax vessels AI plugin

This optional plugin uses the focused TotalSegmentator `lung_vessels` task. It
produces two editable masks: pulmonary vessels and trachea/bronchi. It does not
run the much larger all-organ `total` task.

## Why it is optional

The LungMask plugin is small enough to be the default. TotalSegmentator brings
a larger PyTorch/nnU-Net runtime, so this plugin has its own virtual environment
and is not installed automatically. From `apps/viewer`, install it with:

```sh
./ai-plugins/thorax-vessels/setup.sh
```

Model weights download on the first inference. The worker defaults to CPU on
Apple Silicon because it is the most predictable option within 16 GB unified
memory. To try MPS, start the viewer with `AETHERIS_TOTAL_DEVICE=mps`. If MPS is
not available the worker falls back to CPU. Only one AI job is allowed at a
time, and the worker exits after every inference so model memory is released.

Expected peak memory is about 8 GB. A full chest CT may take several minutes on
CPU. This output is intended for visualization and editing, not diagnosis.

TotalSegmentator and its model weights have their own license terms. Verify
those terms for the intended deployment before distributing or using the
plugin commercially.
