# pipelines.rs

CLI adapter for `pipelines --input routes.jsonl --manifest manifest.json --out
pipelines.1ka`. Dispatches to the shared archive builder. The downloader lives in
scripts/fetch_pipelines.py; this offline operation does no network work. Existing
output files are rejected. Failures propagate to a nonzero CLI exit status.
