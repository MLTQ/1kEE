#!/usr/bin/env python3
"""Regenerate `crates/desktop/src/stellar_catalog.rs` from the Yale Bright Star
Catalogue (BSC5).

The runtime catalogue used to be ~100 hand-written entries. This script expands
it to the full BSC5 (~9,100 stars, complete to about V=6.5 — the naked-eye sky)
while preserving the curated common names that were already in the file, so the
labelled stars keep reading exactly as before.

Usage:

    python3 tools/generate_star_catalog.py            # fetch BSC5 and rewrite
    python3 tools/generate_star_catalog.py --catalog bsc5.dat

Source: VizieR V/50, Hoffleit & Warren, Yale Bright Star Catalogue 5th ed.
The catalogue is a public-domain astronomical reference.

BSC5 is a fixed-width ASCII record; the columns used here (1-indexed, per the
V/50 ReadMe) are:

    1-4     HR number
    5-14    Name (Flamsteed number, Bayer letter, constellation)
    76-90   J2000 position: RAh RAm RAs DE- DEd DEm DEs
    103-107 V magnitude

Rows with a blank position or magnitude are novae/supernovae placeholders and
are skipped.
"""

from __future__ import annotations

import argparse
import gzip
import math
import re
import sys
import urllib.request
from pathlib import Path

BSC5_URL = "https://cdsarc.cds.unistra.fr/ftp/V/50/catalog.gz"

REPO_ROOT = Path(__file__).resolve().parent.parent
CATALOG_RS = REPO_ROOT / "crates" / "desktop" / "src" / "stellar_catalog.rs"

# Greek letters for Bayer designations. BSC5 stores the 3-letter abbreviation.
BAYER = {
    "Alp": "α", "Bet": "β", "Gam": "γ", "Del": "δ", "Eps": "ε", "Zet": "ζ",
    "Eta": "η", "The": "θ", "Iot": "ι", "Kap": "κ", "Lam": "λ", "Mu ": "μ",
    "Nu ": "ν", "Xi ": "ξ", "Omi": "ο", "Pi ": "π", "Rho": "ρ", "Sig": "σ",
    "Tau": "τ", "Ups": "υ", "Phi": "φ", "Chi": "χ", "Psi": "ψ", "Ome": "ω",
}

# A curated name and a BSC5 row are the same star when they fall within this
# many degrees of each other. Bright stars are far enough apart that a tenth of
# a degree cannot produce an ambiguous match.
MATCH_TOLERANCE_DEG = 0.1

# The hand-written catalogue this script replaced carried transcription errors:
# for these five stars the declination was right but the right ascension was off
# by up to 5°, which is far outside any safe positional tolerance. They are
# pinned by HR number instead, each identity corroborated by the magnitude BSC5
# reports. (BSC5 also corrects two magnitudes the old file got wrong: Menkib is
# V=4.04, not 2.39, and Gienah is V=2.59, not 2.75.)
NAME_BY_HR = {
    "5944": "Fang",        # π Sco
    "1228": "Menkib",      # ξ Per
    "7194": "Ascella",     # ζ Sgr
    "4216": "Mu Velorum",  # μ Vel
    "4662": "Gienah",      # γ Crv
}


def fetch_catalog() -> bytes:
    """Download BSC5 and return the decompressed catalogue text."""
    print(f"fetching {BSC5_URL}", file=sys.stderr)
    with urllib.request.urlopen(BSC5_URL, timeout=120) as response:
        return gzip.decompress(response.read())


def parse_curated_names(source: str) -> list[tuple[float, float, float, str]]:
    """Pull (ra_deg, dec_deg, mag, name) out of the existing generated or
    hand-written catalogue so regenerating never loses a common name."""
    pattern = re.compile(
        r"Star\s*\{\s*ra_deg:\s*(-?[\d.]+)\s*,\s*dec_deg:\s*(-?[\d.]+)\s*,"
        r"\s*mag:\s*(-?[\d.]+)\s*,\s*name:\s*\"([^\"]*)\"\s*,?\s*\}"
    )
    found = []
    for match in pattern.finditer(source):
        name = match.group(4)
        if name:
            found.append(
                (float(match.group(1)), float(match.group(2)), float(match.group(3)), name)
            )
    return found


def angular_distance_deg(ra1: float, dec1: float, ra2: float, dec2: float) -> float:
    """Great-circle separation on the celestial sphere, in degrees."""
    p1, p2 = math.radians(dec1), math.radians(dec2)
    delta = math.radians(ra2 - ra1)
    cos_sep = math.sin(p1) * math.sin(p2) + math.cos(p1) * math.cos(p2) * math.cos(delta)
    return math.degrees(math.acos(max(-1.0, min(1.0, cos_sep))))


def designation(name_field: str) -> str:
    """Render the BSC5 name column as a readable Bayer/Flamsteed designation.

    Column layout inside the field: Flamsteed number (1-3), Bayer letter (4-6),
    a superscript index (7), and the constellation abbreviation (8-10).
    """
    if len(name_field) < 10:
        name_field = name_field.ljust(10)
    flamsteed = name_field[0:3].strip()
    bayer_raw = name_field[3:6]
    superscript = name_field[6:7].strip()
    constellation = name_field[7:10].strip()

    if not constellation:
        return ""

    bayer = BAYER.get(bayer_raw, bayer_raw.strip())
    if bayer:
        return f"{bayer}{superscript} {constellation}".replace("  ", " ").strip()
    if flamsteed:
        return f"{flamsteed} {constellation}"
    return ""


def parse_bsc5(text: str) -> list[dict]:
    """Parse BSC5 fixed-width rows into star dictionaries."""
    stars = []
    for line in text.splitlines():
        if len(line) < 107:
            continue

        ra_h, ra_m, ra_s = line[75:77], line[77:79], line[79:83]
        dec_sign, dec_d, dec_m, dec_s = line[83:84], line[84:86], line[86:88], line[88:90]
        magnitude = line[102:107].strip()

        # Blank position or magnitude marks a catalogue placeholder entry.
        if not ra_h.strip() or not dec_d.strip() or not magnitude:
            continue

        try:
            ra_deg = (int(ra_h) + int(ra_m) / 60.0 + float(ra_s) / 3600.0) * 15.0
            dec_deg = int(dec_d) + int(dec_m) / 60.0 + int(dec_s) / 3600.0
            if dec_sign == "-":
                dec_deg = -dec_deg
            mag = float(magnitude)
        except ValueError:
            continue

        stars.append(
            {
                "hr": line[0:4].strip(),
                "ra": ra_deg,
                "dec": dec_deg,
                "mag": mag,
                "name": designation(line[4:14]),
            }
        )
    return stars


def apply_curated_names(stars: list[dict], curated: list[tuple[float, float, str]]) -> int:
    """Overwrite catalogue designations with curated common names where the two
    refer to the same star. Returns the number of names applied."""
    applied = 0
    pinned = set(NAME_BY_HR.values())

    by_hr = {star["hr"]: star for star in stars}
    for hr, name in NAME_BY_HR.items():
        star = by_hr.get(hr)
        if star is None:
            print(f"  warning: pinned HR {hr} ({name}) not in catalogue", file=sys.stderr)
            continue
        star["name"] = name
        applied += 1

    for ra, dec, mag, name in curated:
        if name in pinned:
            continue
        best, best_score = None, None
        for star in stars:
            # Cheap declination reject before the trigonometric distance.
            if abs(star["dec"] - dec) > MATCH_TOLERANCE_DEG:
                continue
            sep = angular_distance_deg(ra, dec, star["ra"], star["dec"])
            if sep > MATCH_TOLERANCE_DEG:
                continue
            # Position alone cannot separate a close visual double — the two
            # α Centauri components sit 0.001° apart — so magnitude breaks the
            # tie and keeps a common name on the component it belongs to.
            score = sep / MATCH_TOLERANCE_DEG + abs(star["mag"] - mag) / 5.0
            if best_score is None or score < best_score:
                best, best_score = star, score
        if best is not None:
            best["name"] = name
            applied += 1
        else:
            print(f"  warning: no BSC5 match for curated name {name!r}", file=sys.stderr)
    return applied


def render_rust(stars: list[dict], named_count: int) -> str:
    """Emit the Rust source for the catalogue."""
    lines = [
        "// @generated by tools/generate_star_catalog.py — do not edit by hand.",
        "//",
        "// Source: Yale Bright Star Catalogue, 5th revised edition (VizieR V/50),",
        "// Hoffleit & Warren. Positions are J2000. Curated common names are",
        "// preserved across regeneration; every other star carries its Bayer or",
        "// Flamsteed designation.",
        "",
        "/// A star record from the Yale Bright Star Catalogue (BSC5).",
        "/// Coordinates are J2000 epoch.",
        "pub struct Star {",
        "    /// Right Ascension in degrees [0, 360).",
        "    pub ra_deg: f32,",
        "    /// Declination in degrees [-90, 90].",
        "    pub dec_deg: f32,",
        "    /// Visual (Johnson V) magnitude; lower is brighter.",
        "    pub mag: f32,",
        "    /// Common, Bayer, or Flamsteed name. Empty when the catalogue",
        "    /// assigns no designation.",
        "    pub name: &'static str,",
        "}",
        "",
        "/// Faintest magnitude the stellar layer draws by default.",
        "///",
        "/// BSC5 is complete to about V=6.5 (8,404 of the entries here), but",
        "/// everything past V=6.0 renders as a minimum-radius, minimum-alpha dot",
        "/// while still costing a projection, so the default stops at 6.0 —",
        "/// 5,080 stars, which is already a dark-sky naked-eye view. Raise it to",
        "/// 6.5 to draw the full catalogue.",
        "///",
        "/// Renderers rely on [`STARS`] being sorted by increasing magnitude so",
        "/// they can break at this value instead of scanning the whole",
        "/// catalogue every frame.",
        "pub const RENDER_MAG_LIMIT: f32 = 6.0;",
        "",
        "/// The Bright Star Catalogue, sorted by increasing magnitude",
        f"/// ({len(stars)} stars, {named_count} of them named).",
        "///",
        "/// Stellar correspondence mapping:",
        "///   Earth latitude  = star declination",
        "///   Earth longitude = star RA (0–180° → 0–180° E; 180–360° → 180–0° W)",
        "pub const STARS: &[Star] = &[",
    ]

    for star in stars:
        name = star["name"].replace("\\", "\\\\").replace('"', '\\"')
        lines.append(
            f'    Star {{ ra_deg: {star["ra"]:8.3f}, dec_deg: {star["dec"]:7.3f}, '
            f'mag: {star["mag"]:5.2f}, name: "{name}" }},'
        )

    lines.append("];")
    lines.append("")
    return "\n".join(lines)


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument(
        "--catalog",
        type=Path,
        help="path to a local BSC5 catalog file (plain or .gz); downloads it when omitted",
    )
    parser.add_argument(
        "--output",
        type=Path,
        default=CATALOG_RS,
        help=f"Rust file to write (default: {CATALOG_RS})",
    )
    args = parser.parse_args()

    if args.catalog:
        raw = args.catalog.read_bytes()
        data = gzip.decompress(raw) if args.catalog.suffix == ".gz" else raw
    else:
        data = fetch_catalog()

    text = data.decode("latin-1")
    stars = parse_bsc5(text)
    if len(stars) < 9000:
        print(f"error: parsed only {len(stars)} stars; catalogue looks wrong", file=sys.stderr)
        return 1
    print(f"parsed {len(stars)} stars", file=sys.stderr)

    curated = []
    if args.output.exists():
        curated = parse_curated_names(args.output.read_text())
        print(f"found {len(curated)} curated names to preserve", file=sys.stderr)

    applied = apply_curated_names(stars, curated)
    print(f"applied {applied} curated names", file=sys.stderr)

    stars.sort(key=lambda star: (star["mag"], star["ra"]))
    named_count = sum(1 for star in stars if star["name"])

    args.output.write_text(render_rust(stars, named_count))
    print(f"wrote {args.output} ({len(stars)} stars, {named_count} named)", file=sys.stderr)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
