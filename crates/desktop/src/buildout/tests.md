# tests.rs

Fixtures cover both bundle representations, missing and malformed tables,
precision, missing MW, jurisdiction-only rules, news-to-record links, changing
asset hashes, cache persistence, retry scheduling and conditional HTTP.
An ignored live check exercises the production downloader on demand.

A headless egui test follows an Earth marker into its campus record and inspects the rendered rule and news details, including related-record links.
