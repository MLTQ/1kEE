# event_radio_tests.rs

## Purpose
Verifies area/category filtering, explicit retargeting, pinned playback context,
late-result cancellation and actual headless egui rendering.

## Contracts
All fixtures are isolated from source adapters, persisted user configuration and
audio output. Tests prime a fresh catalog so no search worker starts.

A pointer-click test exercises the actual Nearby radio button. The opt-in
`event_radio_preview.rs` renders the native window through wgpu for visual review.

The Brief title-bar icon is clicked in ordinary and anchored Follow windows,
checking that it opens radio without closing/collapsing the Brief or playing audio.
