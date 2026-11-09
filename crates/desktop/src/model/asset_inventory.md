# asset_inventory.rs

AppModel queues terrain/OSM inventory scanning at startup and after root/settings
changes. Construction creates only placeholders. The frame poll starts one
worker, checks a channel without waiting, and publishes completed inventories,
runtime-store status and activity messages. Folder creation, terrain directory
counts, OSM schema setup and database inventory reads stay on that worker.
Settings shows scanning until completion. Console messages identify background
scan start and elapsed completion time.

Refresh requests coalesce behind the existing worker. Generation IDs reject old
results even if settings change while the selected root is unchanged. A blocked
USB read cannot hold the UI model lock or start an unbounded set of replacement
workers. Panics/spawn failures become activity errors; explicit refresh can retry.
Dropping the model disconnects publication without joining a blocked worker.

This does not accelerate source reads or change Atlas residency. Event-history
storage and GPU initialization still happen in DashboardApp::new. Subsequent
layer loads continue using their own existing asynchronous pipelines.

Validation on the Arch RTX 4090 workstation (2026-11-09): an isolated release
desktop using real Minkowski Data/SRTM paths and private settings/Derived files
mapped a visible native window in 0.425 seconds. The log had started the asset
scan but had not completed it when the test window appeared. This is a warm
startup measurement, not a cold-boot or complete-terrain-load guarantee. The
existing user desktop and OSM cache builder were left running.
