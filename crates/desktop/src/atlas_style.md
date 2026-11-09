# atlas_style.rs

## Purpose
Own saved Atlas light direction, Sun/studio mode, color richness and relief
contrast independently of source terrain, camera geometry and layer visibility.

## Components
- `AtlasStyle`: backward-compatible serde defaults and finite/clamped controls.
- `light`: studio east/north/up vector, or Earth-fixed Sun direction using the
  existing planetary ephemeris and sidereal clock. Sun time is minute-quantized;
  its optional offset previews daylight without changing the application's clock.

## Contracts
Settings load/save and UI edits normalize values. Direction is geographic, so
camera rotation does not rotate the light with the screen. Light/style edits
only change surface uniforms and never invalidate height-field source caches.
Studio defaults northwest/35 degrees; astronomical Sun is optional.
Unknown serialized light modes fall back to studio illumination.
