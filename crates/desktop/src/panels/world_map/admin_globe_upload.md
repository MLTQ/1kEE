Owns the displayed and pending globe boundary buffers. Lines and optional fill
share the existing per-frame upload budget and are published as a pair. A new
camera view can reuse the displayed pair during preparation and upload. Compatible
in-flight pairs finish before later views so motion cannot starve publication.
An empty replacement also commits. Different sources, level selections or palettes
discard incompatible buffers. Disabling shading immediately drops its buffers.
