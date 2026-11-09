# asset_inventory_tests.rs

Channel-controlled worker regressions hold inventory work blocked while the UI
polls repeatedly. They assert a single worker, coalescing, rejection after root
and same-root settings changes, publication of the latest result, and no repeated
scan after completion. A panic regression verifies error publication and retry.
No real assets, user settings, network, or databases are touched.
