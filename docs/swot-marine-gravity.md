# SWOT marine gravity

1kEE can display the marine **vertical gravity gradient (VGG)** from the Scripps
SWOT project in Globe and Local Earth views. Enable **Layers → SWOT marine
gravity**. It starts off and works offline after preparation, independently of
bathymetry and the detailed-terrain streaming setting.

VGG highlights gravity signatures associated with seamounts, abyssal hills,
ridges and submarine canyons. It measures gravity variation in Eötvös
(1 E = 10⁻⁹ s⁻²), **not water depth or directly surveyed seafloor elevation**.
Blue denotes negative VGG, dark gray zero and orange positive VGG. The fixed
display scale saturates at ±20 E; larger values remain in the prepared grid.
The source's reference-model values over land are preserved; missing cells are
transparent. Geographic coverage is −80°…80° latitude; polar caps remain empty.
One-arcminute sample spacing is not the independent
feature resolution: the original paper reports approximately 8 km resolution.

## Sources

- [Yu, Sandwell & Dibarboure (2024), *Abyssal marine tectonics from the SWOT mission*](https://doi.org/10.1126/science.ads4472).
- [Sandwell et al. (2021), *Gravity field recovery from geodetic altimeter missions*](https://doi.org/10.1016/j.asr.2019.09.011).
- [Scripps SWOT_06 release notes](https://topex.ucsd.edu/pub/global_grav_1min_SWOT/README_SWOT_06.txt)
  request both citations. This September 15, 2026 update uses three years of
  SWOT data and improved removal of time-varying ocean dynamic topography.
- The original paper's [archived dataset](https://doi.org/10.5281/zenodo.12786353)
  is a separate, older release (Zenodo metadata declares CC BY 4.0). 1kEE uses
  the newer Scripps product and preserves its release notes; it does not claim
  that the archive's license applies to the newer files. Raw and prepared grids
  remain on the user's data drive and are not redistributed with the code.

## Preparation on the data drive

Requires Python 3.11+, curl and GDAL with NetCDF support. The user's data root is
`/Volumes/Hilbert`; mount that drive before starting:

```bash
python3 scripts/fetch_swot.py --asset-root /Volumes/Hilbert
```

The source download is approximately 640 MiB and prepared files approximately
461 MiB. Allow about 1.6 GiB including temporary conversion space. Downloads
resume after interruption. Existing prepared directories are never replaced.
Raw data and release notes go in `Data/SWOT/SWOT_06/`; runtime grids, checksums,
source URLs and a processing manifest go in `Derived/swot/`. Configure that
drive under **Settings → Files & Cache** using the existing Data/Derived paths.
This is a static dataset and needs no routine reload. If replacing prepared
files while 1kEE is open, restart the app or toggle the layer off and on.

The globe uses an area-average 4096×2048 overview. Local reads sample at most
1024×1024 pixels from the original one-arcminute grid, with padded reusable
windows and wrapped dateline reads. No whole-world native grid is held in RAM,
and all source discovery, reads and color conversion run off the drawing thread.
The images overlay the sphere/sea-level plane; they do not change contour,
terrain or marker elevations.

## Verification

```bash
python3 -m unittest discover -s scripts -p 'test_fetch_swot.py'
cargo test -p one-thousand-electric-eye-desktop swot_layer
```

The conversion fixture covers scale, NaN/missing masks, polar caps, north-up
registration and overview averaging. Rust tests exercise real row reads,
dateline wrapping, truncated input rejection, palette units and request bounds.
