# mod.rs

The desktop ticks this source once per frame. Enabling it reads the cache on
a worker and schedules a conditional check 24 hours after the previous success.
A stale cache is published immediately while a refresh runs. HTTP and decoding
errors leave that snapshot available and delay the next attempt for one hour.
Disabling prevents new workers. After a data-root change, the source drains the old worker before reading the new path and discards results for the previous root.

The bundled application contains the adapter. Records come from the publisher
or the operator's local cache. The worker wakes egui when it has data to show.
