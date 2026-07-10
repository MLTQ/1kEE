// 1kEE mesh companion — a thin web view that mirrors the host's native globe and
// lets viewers steer it. The native app is the source of truth; we poll its `api`
// upstream for the live picture and POST one-shot commands back. Shared pins ride
// the Gruve session (L3) so every viewer of this tile sees them.
//
// Contract notes:
//  * apiBase("api") resolves to /apps/1kee/__gruve/api over the mesh, or "" (same
//    origin) standalone — no hardcoded localhost anywhere.
//  * joinSession() is a local no-op room when standalone or opened Solo.
//  * Appliers are idempotent renderers: applying a shared pin just draws it; it
//    never re-sends or triggers an action (rule 6).

import { apiBase, isServedByGruve, joinSession } from "./gruve-sdk.js";

const API = apiBase("api", { fallback: "" });

// ── Theme palettes, keyed by the host's active theme label ──────────────────
// Mirrors the native MapTheme so the companion recolours to match the analyst.
const THEMES = {
  TOPO:     { bg: "#0a0e14", sphereA: "#0d1c2c", sphereB: "#05090f", grid: "rgba(120,180,220,.16)", accent: "#58b0d6" },
  PHOSPHOR: { bg: "#060a06", sphereA: "#0a1f12", sphereB: "#020602", grid: "rgba(120,230,150,.16)", accent: "#7fe39a" },
  THERMAL:  { bg: "#0c0814", sphereA: "#1a0f2c", sphereB: "#060410", grid: "rgba(190,150,240,.16)", accent: "#c6a6ff" },
  GHOST:    { bg: "#0c0e10", sphereA: "#1b2228", sphereB: "#080a0c", grid: "rgba(200,210,220,.16)", accent: "#cfd8e0" },
  AKIRA:    { bg: "#070708", sphereA: "#1c0710", sphereB: "#030203", grid: "rgba(120,200,255,.18)", accent: "#46c8ff" },
  SODIUM:   { bg: "#0b0805", sphereA: "#241704", sphereB: "#070401", grid: "rgba(255,190,110,.16)", accent: "#ffbe6e" },
  LUNAR:    { bg: "#08090b", sphereA: "#1a1c20", sphereB: "#050608", grid: "rgba(210,215,225,.15)", accent: "#cdd3dd" },
  MARS:     { bg: "#0d0604", sphereA: "#2a0f06", sphereB: "#080302", grid: "rgba(230,140,90,.16)", accent: "#e88c5a" },
  "MARS DARK": { bg: "#0a0303", sphereA: "#220707", sphereB: "#060101", grid: "rgba(200,70,70,.18)", accent: "#d24a4a" },
};
const SEV = { Critical: "#f25a4a", Elevated: "#ffba49", Advisory: "#7ed0e5" };
const CAM = { idle: "#969696", attempted: "#7ed0e5", reachable: "#75c968", unreachable: "#f25a4a" };

const D2R = Math.PI / 180, R2D = 180 / Math.PI;

// ── Live state ──────────────────────────────────────────────────────────────
const state = {
  view: { lat: 20, lon: 0, body: "earth", theme: "TOPO", local_mode: false },
  show: { events: true, ships: true, flights: true },
  selected: {},
  events: [], cameras: [], tracks: [], flights: [],
  target: null,      // { lat, lon } picked by this viewer
  pin: null,         // { lat, lon } shared across viewers via the session
  layers: { events: true, cameras: true, tracks: true, flights: true },
};

// ── Canvas ──────────────────────────────────────────────────────────────────
const canvas = document.getElementById("globe");
const ctx = canvas.getContext("2d");
let W = 0, H = 0, CX = 0, CY = 0, RAD = 0, DPR = 1;

function resize() {
  DPR = window.devicePixelRatio || 1;
  const r = canvas.getBoundingClientRect();
  W = Math.max(1, Math.round(r.width));
  H = Math.max(1, Math.round(r.height));
  canvas.width = W * DPR;
  canvas.height = H * DPR;
  ctx.setTransform(DPR, 0, 0, DPR, 0, 0);
  CX = W / 2;
  CY = H / 2;
  RAD = Math.min(W, H) * 0.44;
}
window.addEventListener("resize", resize);

// ── Orthographic projection centred on the host's view ──────────────────────
function project(lat, lon) {
  const la = lat * D2R, lo = lon * D2R;
  const la0 = state.view.lat * D2R, lo0 = state.view.lon * D2R;
  const cosc = Math.sin(la0) * Math.sin(la) + Math.cos(la0) * Math.cos(la) * Math.cos(lo - lo0);
  if (cosc < 0) return null; // far hemisphere
  const x = Math.cos(la) * Math.sin(lo - lo0);
  const y = Math.cos(la0) * Math.sin(la) - Math.sin(la0) * Math.cos(la) * Math.cos(lo - lo0);
  return { x: CX + x * RAD, y: CY - y * RAD, front: cosc };
}

// Inverse: screen pixel → lat/lon, or null when outside the disc.
function unproject(px, py) {
  const x = (px - CX) / RAD, y = (CY - py) / RAD;
  const rho = Math.hypot(x, y);
  if (rho > 1) return null;
  const la0 = state.view.lat * D2R, lo0 = state.view.lon * D2R;
  const c = Math.asin(Math.min(1, rho));
  const sinc = Math.sin(c), cosc = Math.cos(c);
  const lat = rho === 0 ? state.view.lat
    : Math.asin(cosc * Math.sin(la0) + (y * sinc * Math.cos(la0)) / rho) * R2D;
  const lon = lo0 + Math.atan2(x * sinc, rho * cosc * Math.cos(la0) - y * sinc * Math.sin(la0));
  let lonDeg = lon * R2D;
  lonDeg = ((lonDeg + 180) % 360 + 360) % 360 - 180;
  return { lat, lon: lonDeg };
}

// ── Render ──────────────────────────────────────────────────────────────────
function palette() { return THEMES[state.view.theme] || THEMES.TOPO; }

function draw() {
  const p = palette();
  ctx.clearRect(0, 0, W, H);

  // Sphere body.
  const grad = ctx.createRadialGradient(CX - RAD * 0.3, CY - RAD * 0.3, RAD * 0.1, CX, CY, RAD);
  grad.addColorStop(0, p.sphereA);
  grad.addColorStop(1, p.sphereB);
  ctx.beginPath();
  ctx.arc(CX, CY, RAD, 0, Math.PI * 2);
  ctx.fillStyle = grad;
  ctx.fill();
  ctx.lineWidth = 1;
  ctx.strokeStyle = p.accent + "55";
  ctx.stroke();

  drawGraticule(p);

  // Vessels & flights first (dense, low priority), then cameras, then events on top.
  if (state.layers.tracks && state.show.ships) drawMovers(state.tracks, "#5fd4c4", 1.4);
  if (state.layers.flights && state.show.flights) drawFlights(state.flights);
  if (state.layers.cameras) drawCameras();
  if (state.layers.events && state.show.events) drawEvents();

  drawPin();
  drawTarget(p);
  drawReticle(p);
}

function drawGraticule(p) {
  ctx.strokeStyle = p.grid;
  ctx.lineWidth = 1;
  // Parallels.
  for (let lat = -60; lat <= 60; lat += 30) strokePath(latLine(lat));
  // Meridians.
  for (let lon = -180; lon < 180; lon += 30) strokePath(meridian(lon));
}

function latLine(lat) {
  const pts = [];
  for (let lon = -180; lon <= 180; lon += 4) pts.push(project(lat, lon));
  return pts;
}
function meridian(lon) {
  const pts = [];
  for (let lat = -90; lat <= 90; lat += 4) pts.push(project(lat, lon));
  return pts;
}
function strokePath(pts) {
  ctx.beginPath();
  let pen = false;
  for (const pt of pts) {
    if (!pt) { pen = false; continue; }
    if (!pen) { ctx.moveTo(pt.x, pt.y); pen = true; }
    else ctx.lineTo(pt.x, pt.y);
  }
  ctx.stroke();
}

function dot(lat, lon, color, r) {
  const pt = project(lat, lon);
  if (!pt) return null;
  ctx.beginPath();
  ctx.arc(pt.x, pt.y, r, 0, Math.PI * 2);
  ctx.fillStyle = color;
  ctx.fill();
  return pt;
}

function drawMovers(list, color, r) {
  for (const m of list) dot(m.lat, m.lon, color, r);
}

function drawFlights(list) {
  ctx.fillStyle = "#ffcf6b";
  for (const f of list) {
    const pt = project(f.lat, f.lon);
    if (!pt) continue;
    ctx.beginPath();
    ctx.moveTo(pt.x, pt.y - 2.4);
    ctx.lineTo(pt.x - 2, pt.y + 2);
    ctx.lineTo(pt.x + 2, pt.y + 2);
    ctx.closePath();
    ctx.fill();
  }
}

function drawCameras() {
  for (const c of state.cameras) {
    const pt = project(c.lat, c.lon);
    if (!pt) continue;
    ctx.fillStyle = CAM[c.status] || CAM.idle;
    ctx.fillRect(pt.x - 2, pt.y - 2, 4, 4);
  }
}

function drawEvents() {
  for (const e of state.events) {
    const sel = state.selected.event === e.id;
    const color = SEV[e.severity] || "#ccc";
    const pt = dot(e.lat, e.lon, color, sel ? 5 : 3.5);
    if (!pt) continue;
    ctx.beginPath();
    ctx.arc(pt.x, pt.y, sel ? 9 : 6.5, 0, Math.PI * 2);
    ctx.strokeStyle = color + (sel ? "ff" : "88");
    ctx.lineWidth = sel ? 2 : 1;
    ctx.stroke();
  }
}

function drawReticle(p) {
  // The host's view centre sits at the middle of the disc by construction.
  ctx.strokeStyle = p.accent + "cc";
  ctx.lineWidth = 1;
  ctx.beginPath();
  ctx.moveTo(CX - 9, CY); ctx.lineTo(CX - 3, CY);
  ctx.moveTo(CX + 3, CY); ctx.lineTo(CX + 9, CY);
  ctx.moveTo(CX, CY - 9); ctx.lineTo(CX, CY - 3);
  ctx.moveTo(CX, CY + 3); ctx.lineTo(CX, CY + 9);
  ctx.stroke();
}

function drawTarget(p) {
  if (!state.target) return;
  const pt = project(state.target.lat, state.target.lon);
  if (!pt) return;
  ctx.strokeStyle = p.accent;
  ctx.lineWidth = 1.5;
  ctx.beginPath();
  ctx.arc(pt.x, pt.y, 7, 0, Math.PI * 2);
  ctx.stroke();
}

function drawPin() {
  if (!state.pin) return;
  const pt = project(state.pin.lat, state.pin.lon);
  if (!pt) return;
  ctx.fillStyle = "#ff5db1";
  ctx.beginPath();
  ctx.moveTo(pt.x, pt.y);
  ctx.lineTo(pt.x - 5, pt.y - 11);
  ctx.lineTo(pt.x + 5, pt.y - 11);
  ctx.closePath();
  ctx.fill();
  ctx.beginPath();
  ctx.arc(pt.x, pt.y - 11, 4, 0, Math.PI * 2);
  ctx.fill();
}

function frame() {
  draw();
  requestAnimationFrame(frame);
}

// ── Theme application to the DOM chrome ─────────────────────────────────────
function applyTheme() {
  const p = palette();
  const root = document.documentElement.style;
  root.setProperty("--bg", p.bg);
  root.setProperty("--accent", p.accent);
  root.setProperty("--grid", p.grid);
}

// ── Polling the host's api upstream ─────────────────────────────────────────
// Conditional polling: the host stamps every API response with the slice's
// generation counter (X-Gen). We remember it per path and send it back as
// `?gen=`; when nothing changed the host answers 304 with no body, and we
// return null so the caller keeps its state — no JSON parse, no re-render.
const gens = {};
async function getJSON(path) {
  const seen = gens[path];
  const url = `${API}/${path}` + (seen !== undefined ? `?gen=${seen}` : "");
  const r = await fetch(url, { cache: "no-store" });
  if (r.status === 304) return null; // unchanged since last poll
  if (!r.ok) throw new Error(`${path} ${r.status}`);
  const gen = r.headers.get("x-gen");
  if (gen !== null) gens[path] = gen;
  return r.json();
}

const link = document.getElementById("link-state");
function setLink(ok) {
  link.textContent = ok ? "linked" : "waiting";
  link.className = "pill " + (ok ? "live" : "waiting");
}

async function pollState() {
  try {
    const s = await getJSON("state");
    if (s) {
      const themeChanged = s.view.theme !== state.view.theme;
      const selChanged = s.selected.event !== state.selected.event;
      state.view = s.view;
      state.show = s.show;
      state.selected = s.selected;
      if (themeChanged) applyTheme();
      if (selChanged) renderEventList(); // highlight follows host selection
      renderHostCard(s.counts);
    }
    setLink(true);
  } catch {
    setLink(false);
  }
}

async function pollLists() {
  // Each slice comes back null when the host says 304 — keep what we have and
  // only re-render the lists whose data actually changed.
  try {
    const [ev, cam, trk, flt] = await Promise.all([
      getJSON("events"), getJSON("cameras"), getJSON("tracks"), getJSON("flights"),
    ]);
    if (ev) {
      state.events = ev;
      renderEventList();
      setCount("c-events", state.events.length);
    }
    if (cam) {
      state.cameras = cam;
      setCount("c-cameras", state.cameras.length);
    }
    if (trk) {
      state.tracks = trk.items || [];
      setCount("c-tracks", trk.total ?? state.tracks.length);
    }
    if (flt) {
      state.flights = flt.items || [];
      setCount("c-flights", flt.total ?? state.flights.length);
    }
  } catch { /* host briefly unreachable — keep last picture */ }
}

function setCount(id, n) { const el = document.getElementById(id); if (el) el.textContent = n; }

function renderHostCard(counts) {
  const v = state.view;
  document.getElementById("host-center").textContent = `${fmt(v.lat)}, ${fmt(v.lon)}`;
  document.getElementById("host-body").textContent = v.body.toUpperCase() + (v.local_mode ? " · local" : "");
  document.getElementById("host-theme").textContent = v.theme;
  document.getElementById("readout").textContent =
    `centre  ${fmt(v.lat)} ${fmt(v.lon)}\nbody    ${v.body}`;
}

function fmt(x) { return (x >= 0 ? "+" : "") + x.toFixed(2) + "°"; }

function renderEventList() {
  const host = document.getElementById("event-list");
  host.innerHTML = "";
  for (const e of state.events) {
    const div = document.createElement("div");
    const cls = e.severity === "Critical" ? "crit" : e.severity === "Elevated" ? "elev" : "adv";
    div.className = "ev " + cls + (state.selected.event === e.id ? " sel" : "");
    div.innerHTML =
      `<div class="ev-title"></div><div class="ev-meta"></div>`;
    div.querySelector(".ev-title").textContent = e.title;
    div.querySelector(".ev-meta").textContent = `${e.severity} · ${e.location_name} · ${e.occurred_at}`;
    div.addEventListener("click", () => selectEvent(e));
    host.appendChild(div);
  }
}

// ── Sending commands to the host ────────────────────────────────────────────
async function command(body) {
  try {
    await fetch(`${API}/command`, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify(body),
    });
  } catch { /* host not reachable; ignore */ }
}

function selectEvent(e) {
  state.target = { lat: e.lat, lon: e.lon };
  updateTargetCard();
  command({ type: "select_event", id: e.id });   // steer host to it
  setPin({ lat: e.lat, lon: e.lon });            // and share it with viewers
}

// ── Shared pin via the Gruve session (L3) ───────────────────────────────────
const session = joinSession({ onPeers: (n) => setCount("peer-count", n) });
// Remote truth only: another viewer dropped a pin → draw it. Idempotent.
session.state.subscribe((key, value) => {
  if (key === "pin") state.pin = value;
});

function setPin(pt) {
  state.pin = pt;                 // local echo (our own set() won't fire our subscriber)
  session.state.set("pin", pt);   // broadcast to other viewers
}

// ── Target picking + buttons ────────────────────────────────────────────────
const btnSteer = document.getElementById("btn-steer");
const btnPin = document.getElementById("btn-pin");

canvas.addEventListener("click", (ev) => {
  const r = canvas.getBoundingClientRect();
  const ll = unproject(ev.clientX - r.left, ev.clientY - r.top);
  if (!ll) return;
  state.target = ll;
  updateTargetCard();
});

function updateTargetCard() {
  const t = state.target;
  document.getElementById("target-coord").textContent = t ? `${fmt(t.lat)}, ${fmt(t.lon)}` : "click the globe";
  btnSteer.disabled = !t;
  btnPin.disabled = !t;
}

btnSteer.addEventListener("click", () => {
  if (state.target) command({ type: "focus", lat: state.target.lat, lon: state.target.lon });
});
btnPin.addEventListener("click", () => {
  if (state.target) setPin({ lat: state.target.lat, lon: state.target.lon });
});

// Layer toggles.
for (const key of ["events", "cameras", "tracks", "flights"]) {
  const box = document.getElementById("lyr-" + key);
  box.addEventListener("change", () => { state.layers[key] = box.checked; });
}

// ── Boot ────────────────────────────────────────────────────────────────────
document.getElementById("standalone-note").textContent =
  isServedByGruve() ? "served over the mesh" : "standalone — open via the Gruve lobby for multiplayer";

resize();
applyTheme();
frame();
pollState();
pollLists();
setInterval(pollState, 1000);
setInterval(pollLists, 2500);
