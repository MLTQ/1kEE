# celestial_gpu_tests.rs

## Purpose
Exercise the real scene batch builder and native GPU pipeline while switching
Earth → Moon → Mars → Earth, then optionally render actual cached alien terrain.

## Coverage
- A shared Arc/tile ID must produce distinct body versions and correct colors:
  500 m lines are major on Earth, minor on Moon/Mars; ±1,000 m remain major.
- Pixel positions use the shared projection, including negative elevations.
- Workers and bounded GPU uploads must complete without CPU contour drawing.
- `ONEKEE_CELESTIAL_CACHE_ROOT` enables immutable, read-only SQLite fixtures from
  `lunar_focus_cache.sqlite` and `mars_ctx_cache.sqlite`. No acquisition/builds run.
- Neighboring source tiles use the production core compositor. Every surviving
  segment must reach the GPU; readback must show terrain with occlusion off/on.
- Offscreen PNGs go to `/tmp/1kee-celestial-{moon,mars}-{false,true}.png`.
- The ignored hardware test shares the isolated occlusion harness. It never
  opens a window or changes the app, settings, source caches or runtime records.
