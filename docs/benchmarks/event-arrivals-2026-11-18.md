# Local Earth contour timing during Follow Events

On the RTX 4090, the center tile at the Dead Sea was on screen before the
camera arrived in all three cold runs. The first complete visible contour view
appeared about four seconds into the 30-second dwell.

| Draw milestone | Previous build, from selection | Updated build, from selection | Previous build, from arrival | Updated build, from arrival |
| --- | ---: | ---: | ---: | ---: |
| Destination center tile | 8.04 s | 4.19 s | 3.56 s after | 0.29 s before |
| Center and eight neighbors | 11.51 s | 7.06 s | 7.03 s after | 2.58 s after |
| Complete visible contours | 14.77 s | 8.47 s | 10.30 s after | 3.99 s after |

Each cell is the median of three runs. This comparison used `81784f3` and
`bf34b0d`.
The flight took about 4.48 seconds from Ankara to the Dead Sea, reaching Local
zoom 9.5. Its final source grid had 81 cells. At the complete-view milestone,
34–35 nonempty tiles intersected the rotating view. The previous build also
completed 23–24 terrain jobs outside the destination grid. The updated build
completed zero. In `bf34b0d` the flight started up to 25 destination tiles,
read existing terrain along the flight path, then built the full arrival view.

The ignored `cold_follow_event_arrival` test uses the event flight controller,
Local contour scheduler, geographic compositor and GPU tile upload. It draws
at 960 × 540 and checks for contour pixels at the center and neighborhood
milestones. The complete frame requires resolved viewport progress and a
published GPU batch for each visible nonempty tile. The 16 ms polling delay
and a GPU wait per frame are included. Other map layers, occlusion, the desktop
compositor and OBS are outside this measurement.

Run one test per process with isolated settings, `ONEKEE_GPU='RTX 4090'`,
and `ONEKEE_SRTM_ROOT` pointing to native GL1 files. Each run used a fresh
tile database on the internal SSD and requested Linux file-page eviction for
the SRTM files around the route on the external USB drive. The order was
updated, previous, previous, updated, updated, previous.

## Tiraspol and the external terrain drive

The Ankara flight targeted central Tiraspol at 46.84275° N, 29.62840° E.
The original event coordinates and preceding stop were unavailable. The final
grid was 81 native Earth tiles centered on bucket `4/189/120`. [GeoNames](https://www.geonames.org/617239/tiraspol.html)

The workstation's external native contour database returned `SQLITE_CORRUPT`
on an immutable schema read. Its separate repair store contained 63
checkpointed Tiraspol tiles (177 MiB). The missing 18 formed the eastern two
columns, outside the previous flight's 25-tile prefetch. The original archive
and operator's cache were inspected read-only.

The new scheduler admits missing tiles across the selected arrival footprint,
center first. During native prefetch it stops scanning as soon as the terrain
worker slots fill. Earth selection checks each replacement manifest once per
pass. The display uses each tile's physical database path to look up its
decoded bounds, the same key the reader uses to discard offscreen geometry.

Tiraspol and Dead Sea timings below are medians of three independent flights
per row, in seconds relative to camera arrival. Negative values mean the tile
was drawn during flight. Each flight used the RTX 4090 and the renderer and
completion checks described above.

| Destination and cache at selection | Center | Center and eight neighbors | Complete visible contours |
| --- | ---: | ---: | ---: |
| Tiraspol, external drive with 63 repaired tiles | −0.09 s | 0.65 s | 3.64 s |
| Tiraspol, empty per-tile cache on external drive | 0.59 s | 1.02 s | 9.81 s |
| Tiraspol, empty per-tile cache on internal SSD | −0.37 s | 0.34 s | 0.45 s |
| Tiraspol, empty single database on internal SSD | −0.39 s | 0.32 s | 0.33 s |
| Dead Sea, empty per-tile cache on external drive | 1.27 s | 2.56 s | 22.61 s |
| Dead Sea, empty single database on internal SSD | −0.12 s | 3.39 s | 4.04 s |

The two empty per-tile Tiraspol configurations used the same source TIFFs on the external
drive and the same repair-directory layout. Changing only the repair output
volume moved the median complete frame from 9.81 seconds after arrival to
0.45 seconds after arrival. The RTX 4090 used the native Vulkan SRTM compute
path for these builds.

For the Tiraspol replay with 63 existing tiles, each run used disposable
copies of those databases on the external drive and a small unreadable
primary database to trigger the same recovery path. It used the sibling
repair-directory layout and the workstation's 24 GiB contour VRAM setting.
Both the cold-cache and copied-cache runs requested Linux page eviction for
the source TIFFs along the route. The copied databases were flushed and their
file pages were also requested for eviction. Fresh-cache controls used the
isolated test executable's default contour VRAM setting.

With the progress fix and the previous 25-tile prefetch, zero of the 18
missing tiles finished building in flight. The complete view appeared
5.41 seconds into the dwell. With the full destination footprint, all 18
finished building before arrival in all three runs. Visible coverage passed
3.62–3.75 seconds into the dwell. Unmodified `bf34b0d` timed out 90 seconds
after selection with per-tile storage. In the copied-cache run, at 10 seconds
the old code showed 63/81 ready, with zero pending builds and all 33 decoded
tiles displayed. That timeout measures a stuck progress indicator. A timing
for the first fully drawn frame on the old build cannot be inferred from it.

The ignored `cold_follow_tiraspol_arrival` test shares the Dead Sea harness.
To use a fresh repair cache, point `ONEKEE_TERRAIN_REPAIR_ROOT` at an empty
disposable directory on the volume being tested. Use a test executable with
isolated settings. The flight fixture excludes OBS, desktop composition and
other enabled layers. Sustained broadcast frame pacing is tracked separately.

## Frame cost as coverage history fills

The ignored `event_region_progress_with_coverage_history` test computes progress
for an 81-cell view 2,000 times per process. With 12,288 out-of-view coverage
records across the three source indexes, the previous build averaged 588 µs
per update and the updated build averaged 7.7 µs. With empty indexes the
figures were 2.2 µs and 0.74 µs. These are medians of three process means on
the same CPU core. This measures the progress calculation in isolation.
End-to-end frame pacing during a sustained Local broadcast is tracked in
`1kee-1073`.
