# Streaming native apps over Gruve — difficulties and paths forward

**Status:** design note / proposal, written from a concrete integration attempt
(putting the native Rust app **1kEE** — an egui/wgpu desktop globe — onto a Gruve
mesh). Intended to be lifted into the **gruve-kit** or **gruve agent**, not kept
per-app.

**TL;DR.** A native (non-web) app can announce itself and expose data over Gruve
today, but it cannot *show its real UI* to mesh viewers — there's no served HTML to
proxy. The honest fix is to stream the app's rendered surface as video. Doing that
*correctly* runs into one hard architectural fact: **the agent owns transport
(WireGuard), and the contract forbids apps from doing their own peering** — which is
exactly why "just use WebRTC" doesn't work over the mesh. The capture and encode
pieces are feasible per-app, but the *transport, input-return, compositing, and
consent* pieces are general and should live in Gruve. This note lays out the design
space and recommends a phased home for it.

---

## 1. The problem

Gruve serves apps over HTTP and composites multiplayer (cursors, whiteboard, shared
state) on top. That model is perfect for web apps. For a **native GUI** (egui/iced/
Qt/Unity/SDL/a game engine) there is no HTTP UI to serve, so the kit's current
guidance is one of:

1. **Compile the UI to web** (e.g. eframe → WASM/WebGPU) and split native-only logic
   behind an HTTP upstream.
2. **Companion web view** — a separate web UI that talks to the native backend over
   an upstream.
3. **Service-only** — expose a capability at `/svc/<id>/`, no UI.

We built #2 for 1kEE. It works and is genuinely useful (friends see the live event/
camera/vessel picture, can steer the host, share pins). **But it is not the app.**
The native app renders a custom GPU globe — coastlines, terrain, contours, beams —
and the companion is a hand-drawn 2D mirror of the *data*, so it looks like "an
entirely different app." For a large class of native apps (3D tools, DAWs, CAD,
games, data-viz engines), the rendered surface *is* the product. Reimplementing it
in the browser (#1) is a multi-month rewrite per app; mirroring data (#2) loses the
thing people came for.

So the missing capability is: **let a native app present its actual rendered surface
to mesh viewers, with input flowing back.** Pixel/framebuffer streaming. This is a
solved genre off-mesh (VNC, Parsec, cloud gaming, WebRTC screen-share), but on Gruve
it collides with the dispatch architecture in instructive ways.

---

## 2. What the agent actually gives an app (verified)

From the kit docs plus inspecting the `gruve` binary:

- The agent is built on a **Tailscale/WireGuard** data plane — `magicsock`, `DERP`
  relays, `udprelay/endpoint`, `relayManager`, peer-relay packet counters all appear
  in the binary. The mesh transport is real WireGuard with DERP fallback.
- The surfaces it exposes **to apps** are narrow and all localhost:
  - `POST /gruve/announce` — discovery/heartbeat (L1).
  - HTTP **upstreams** — `…/apps/<id>/__gruve/<name>/…` proxied to a declared local
    port (L2). Header `X-Gruve-Svc-Hop` marks proxied hops.
  - A **session WebSocket** — `…/gruve/session/<id>` — a small last-write-wins
    key/value room with cursors/whiteboard, replayed to late joiners (L3).
  - Services at `/svc/<id>/`.
- **Contract rule 5 (DESIGN-FOR-GRUVE):** *"The app never networks beyond localhost
  + the SDK. No peer addresses, no tailnet IPs, no connection management in app
  code. The agent owns transport and identity."*

That last point is the crux. The agent has *exactly* the substrate a low-latency
media path wants (WireGuard + DERP), but apps are deliberately walled off from it.

---

## 3. Why the obvious approaches fail (and which parts are fine)

### Capture — **fine.**
eframe/egui 0.31's wgpu backend supports framebuffer capture:
`ctx.send_viewport_cmd(ViewportCommand::Screenshot(..))` → `egui::Event::Screenshot {
image }` next frame (confirmed in `egui-wgpu` 0.31). Most native toolkits/engines
have an equivalent readback or can render to an offscreen target. **No blocker here**,
though GPU→CPU readback every frame has a cost (throttle; capture only when viewers
are present).

### Encode — **feasible, with caveats.**
- **MJPEG** (motion-JPEG): trivial (`image` crate), no patent/dep friction, plays in
  a plain `<img>` with zero client JS. High bandwidth, no inter-frame compression.
- **H.264** via `openh264` (vendored C++, builds from source with `cc` — verified it
  *starts* compiling cleanly on macOS arm64). Real compression; patent caveat; every
  host bundles a C++ encoder build. Decode side is easy via **WebCodecs**
  `VideoDecoder`.
- **Hardware encoders** (VideoToolbox/NVENC/VAAPI): best quality/CPU, platform-
  specific. Wasteful to reimplement per app.

> Environmental note from the attempt: the H.264 spike halted on **`No space left on
> device`** (host had ~340 MiB free), not on any code problem. The encoder vendoring
> works; that machine just couldn't fit the C++ object files. Disk pressure is a real
> operational hazard for *any* per-host source-built encoder — an argument for the
> agent owning a single shared/hardware encoder (see §5).

### Transport — **this is where it actually breaks.**
- **True WebRTC** (browser ⇄ host `RTCPeerConnection`): negotiates its own ICE/UDP
  path and **bypasses the agent**. Over the mesh the viewer can only reach the host
  *through* the agent's WireGuard tunnel, which WebRTC knows nothing about, and there
  is no app-facing STUN/TURN/signalling. It also violates rule 5 directly. Net: real
  WebRTC works only standalone / same-LAN — i.e. *not the multiplayer case.* This is
  the dead end people hit first.
- **HTTP upstream (chunked response)** — one-way video as a long-lived
  `multipart/x-mixed-replace` (MJPEG) or a chunked H.264 Annex-B stream fed to
  WebCodecs via `fetch().body`. **Guaranteed to traverse the agent** (upstreams are
  HTTP). Best available carrier today. No back-channel.
- **WebSocket on an upstream** — would carry H.264 chunks bidirectionally, but it's
  **unverified** whether the agent proxies a WS upgrade on `__gruve/<upstream>` paths
  (modern Go reverse proxies usually do; needs a test). The *session* WS is the wrong
  channel — it's a tiny LWW room, not a video pipe; flooding it with frames would
  fight cursors/state and the host-ordered relay.

### Input return — **general, unsolved per-app.**
Remote pointer/keyboard must come back and be injected into the native event loop
(synthesize `egui::Event`/`RawInput`, or inject at the windowing layer). This is
per-toolkit glue, plus arbitration (who's driving among N viewers), plus consent
(host grants control), plus the Solo/Together doors. Rule 6 ("appliers are
idempotent renderers, never simulate input") is about *shared state* — genuine remote
control is a different, explicit mode, but it needs first-class handling so every app
doesn't reinvent it unsafely.

---

## 4. Why this must not live per-app

Every native app that wants this would otherwise reimplement: capture glue, an
encoder build, a framing/streaming endpoint, a JS decode/draw client, an input-return
channel, viewer-presence gating, control arbitration, and consent UX. That's a large,
security-sensitive surface to copy-paste. The parts that are genuinely app-specific
are small (how to grab a frame, how to inject an event into *this* toolkit); the rest
is identical everywhere and is exactly the kind of thing Gruve already centralizes
for cursors and shared state.

---

## 5. Paths forward

Three homes, increasing in power and in how much of Gruve they touch.

### Path A — Kit SDK helper (no agent change) — *ship first*
A `gruve-surface` helper in the kit (Rust + a JS client) that standardizes:
- a **frame sink**: the app hands it captured RGBA frames (+ size, timestamp);
- an **encoder**: MJPEG by default (zero friction), optional H.264 feature;
- a **carrier**: a chunked-HTTP endpoint on the app's existing `api` upstream
  (`GET …/__gruve/api/surface`) — works through today's agent;
- a **JS player**: `<img>` for MJPEG, WebCodecs for H.264, that drops into a
  companion page and composites the app's own data overlay on top.

Pros: works today, on-contract (localhost + SDK only), incremental. Cons: per-host
encoder; one-way only (pair with the existing command channel for coarse control);
limited by what the HTTP upstream can do for latency/bitrate adaptation.

**Recommended minimal first cut:** MJPEG over chunked HTTP, capture-on-demand,
server-side quality/rate adaptation. It immediately turns "an entirely different app"
into "the real globe, live." H.264/WebCodecs is a drop-in carrier/codec upgrade
behind the same helper once §3's WS-or-chunked question is settled.

### Path B — Agent-relayed media channel — *the architecturally correct home*
Make **surface streaming a first-class agent primitive**, the way cursors and shared
state already are:
- an app announces a **surface source** (e.g. `announce({ surface: true })` or a typed
  upstream), handing the agent encoded frames over localhost;
- the **agent owns the media transport** over its *existing WireGuard/DERP plane* —
  the very substrate WebRTC wanted but apps aren't allowed to touch. The agent can run
  the WebRTC/RTP peer (or a custom RTP-over-WireGuard path) *on the app's behalf*,
  keeping rule 5 intact (the app still only talks localhost);
- the **lobby composites** the video like it composites cursors, with the Solo/
  Together doors and the 🔊 listen-along gesture-gate for audio;
- a **standard input-return channel** (sibling to the cursor channel) carries
  pointer/keyboard back, with built-in control arbitration and an explicit host grant.

Pros: any app, any language, consistent UX; transport/codec/congestion-control solved
*once*, centrally; can use **hardware encoders** in the agent instead of N per-host
C++ builds (also sidesteps the disk-pressure hazard from §3); true low-latency
adaptive video **on-contract**. Cons: the biggest change to Gruve — a media stack in
the agent and a compositor path in the lobby.

### Path C — First-class native-app runner (out of scope, noted)
Gruve ships a thin host-side runner that captures a registered window and streams it,
so even apps with *no* Gruve integration can be shared (à la "share this window").
Strictly more than the above; mentioned for completeness.

---

## 6. Recommendation

1. **Kit, now (Path A):** add `gruve-surface` with **MJPEG-over-chunked-HTTP** and a
   tiny JS player. Document a per-toolkit "how to grab a frame" appendix (egui first,
   since it's proven). This unblocks every native app immediately and is fully on
   today's contract.
2. **Settle the carrier question:** test whether the agent proxies a **WebSocket
   upgrade on upstream paths**. If yes, offer WS as the low-latency carrier; if no,
   standardize on chunked HTTP + WebCodecs for H.264. Either way, keep the codec
   behind the helper so apps don't care.
3. **Agent, next (Path B):** design the **agent-owned media channel** over the
   existing WireGuard/DERP plane, with a standard **input-return + control-grant**
   channel and lobby compositing. This is the only route to real WebRTC-class video
   over the mesh without breaking rule 5, and it's where hardware encoding and
   adaptation belong.

### Open questions for the Gruve rebuild
- Does (should) the upstream proxy pass WebSocket upgrades? Binary frames on upstream
  HTTP?
- Is there appetite for the agent to host a media transport (WebRTC/RTP) on apps'
  behalf over WireGuard/DERP? That's the unlock.
- Standard **input event schema** + **control arbitration** + **host consent** model
  for "a viewer is driving my app."
- **Consent/scoping & privacy:** stream only the app's own surface (never the whole
  screen/other monitors/notifications); audio stays under the existing gesture-gate.
- Codec/quality policy: MJPEG floor everywhere; H.264 where WebCodecs is present;
  hardware-encode in the agent when available.

---

## 7. What exists in 1kEE today (reference implementation of Path-A's data half)

The companion already demonstrates the on-contract pieces a surface stream would sit
beside: an embedded localhost HTTP server announced via the SDK (L1), a JSON `api`
upstream (L2), a viewer→host command channel (steer/select), and L3 shared pins —
all degrading silently with no agent/port/viewers. Code: `crates/desktop/src/gruve/`
(see `gruve/mod.md`). Adding a `surface` endpoint there is the natural prototype
site for Path A once the kit helper exists — but the encode/transport/input/consent
machinery should be the kit's or the agent's, not 1kEE's.
