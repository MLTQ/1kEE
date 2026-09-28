# pipeline_pick_tests.rs

Compares BVH hits against exhaustive projected-segment picking across rotation,
zoom/near-plane changes, dateline routes and coincident independent pipelines.
Pins whole-segment horizon rejection so hidden endpoints cannot yield a tooltip.
The ignored real-data check targets Siri–Mobarak in the Gulf, builds the index
for the installed overview, and measures segment tests over 160 mouse positions.

Synthetic cameras use explicit geographic focus to avoid app startup/settings dependencies.
