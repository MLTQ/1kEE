# refresh.rs

Checks the public page with its previous ETag. HTTP 304 revalidates the
snapshot. A changed page can still reference the same immutable data assets.
Otherwise the worker tries the page's bounded list of static scripts until
the campus, rule and news tables validate together. Requests and response
sizes are bounded. News is ordered newest first.

One atomic cache record contains all three tables, contributing asset paths,
the page validator and the time of the last successful check. A failed refresh
cannot advance the validator or replace the cache.
