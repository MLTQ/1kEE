# admin_source.rs

Owns read-only background cache observation and one explicit desktop PBF import.
Polling every three seconds fingerprints level-file sizes and modification times;
file reads, metadata and build-lock probes never run on the render thread. New or
atomically replaced files invalidate the region snapshot automatically in either
map view. A chosen cache folder overrides normal data-root discovery exactly.

PBF loading uses the shared cache-builder library on a named worker. Source and
output are chosen explicitly. An active compact-index lock attaches to the existing
output through the cache watcher; no second writer starts. Errors/panics end
the busy state and remain visible, with scan checkpoints available for a retry.
Background work does not depend on the drawer staying open. Choices are session
state; closing the app ends its own extraction, which can resume from scan checkpoints.
