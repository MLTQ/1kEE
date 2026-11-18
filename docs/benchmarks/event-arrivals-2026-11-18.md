# Local Earth contour timing during Follow Events

On the RTX 4090, the center tile at the Dead Sea was on screen before the
camera arrived in all three cold runs. The first complete visible contour view
appeared about four seconds into the 30-second dwell.

| Draw milestone | Previous build, from selection | Updated build, from selection | Previous build, from arrival | Updated build, from arrival |
| --- | ---: | ---: | ---: | ---: |
| Destination center tile | 8.04 s | 4.19 s | 3.56 s after | 0.29 s before |
| Center and eight neighbors | 11.51 s | 7.06 s | 7.03 s after | 2.58 s after |
| Complete visible contours | 14.77 s | 8.47 s | 10.30 s after | 3.99 s after |

Each cell is the median of three runs. The previous build was `81784f3`.
The flight took about 4.48 seconds from Ankara to the Dead Sea, reaching Local
zoom 9.5. Its final source grid had 81 cells. At the complete-view milestone,
34–35 nonempty tiles intersected the rotating view. The previous build also
completed 23–24 terrain jobs outside the destination grid. The updated build
completed zero. It starts with up to 25 destination tiles during flight,
reads existing terrain along the flight path, then builds the full arrival view.

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

## Frame cost as coverage history fills

The ignored `event_region_progress_with_coverage_history` test computes progress
for an 81-cell view 2,000 times per process. With 12,288 out-of-view coverage
records across the three source indexes, the previous build averaged 588 µs
per update and the updated build averaged 7.7 µs. With empty indexes the
figures were 2.2 µs and 0.74 µs. These are medians of three process means on
the same CPU core. This measures the progress calculation in isolation.
End-to-end frame pacing during a sustained Local broadcast is tracked in
`1kee-1073`.
