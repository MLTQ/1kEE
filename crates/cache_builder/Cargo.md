# Cargo.toml

Declares the shared cache-construction library and companion GUI/CLI. Workspace
cell-format and tile-archive dependencies define the existing cache contracts.
ring supplies SHA-256 for bounded source-identity fingerprints; it already appears
in the workspace lockfile through the desktop TLS stack. No network is used by
the compact PBF index or its fingerprint validation.
