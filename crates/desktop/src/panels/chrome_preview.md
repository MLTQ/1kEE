# chrome_preview.rs

Opt-in GPU rendering of the actual rail, every Settings section and the
Layers sidebar. Set ONEKEE_CHROME_PREVIEW to export PNGs. Uses
synthetic records from chrome_tests; no live source poll or settings save occurs.

The rail preview shows Meander and Follow Events directly, without a Cinematic toggle.
The Atlas day and night previews render the Appearance window and map rail at
opposite longitudes at the same instant.
The dusk preview samples the active fade 0.3 seconds after moving to the night side.
The `follow-events` preview shows the dedicated Settings dwell-time section.
