# event_radio_preview.rs

## Purpose
Opt-in GPU rendering of the actual Nearby radio window in populated, empty and
Add stream states, independent of operational events or radio sources.

## Contracts
`radio_gpu_preview` is ignored by default. It writes 1024×768 PNGs to
`target/radio-qa` or `ONEKEE_RADIO_PREVIEW`; no network requests or audio occur.
