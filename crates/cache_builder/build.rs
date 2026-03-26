// build.rs — downloads Natural Earth 110m country outlines once for the bbox map widget.
//
// The output file is written to `assets/ne_110m_countries.json` (relative to this
// crate root).  It is committed alongside the source so the build works offline
// after the first successful fetch.  If the file already exists the download is
// skipped entirely so incremental builds are free.
//
// We deliberately use only the Rust standard library here (no reqwest/ureq) to
// avoid adding a heavy build-time dependency.  `std::net::TcpStream` + a minimal
// HTTP/1.1 GET with TLS via `rustls-native-certs` would be one option, but the
// simplest portable approach that needs no extra crates is just shelling out to
// `curl` or `wget` if available and falling back gracefully if not.

use std::path::PathBuf;
use std::process::Command;

const NE_URL: &str = "https://raw.githubusercontent.com/nvkelso/natural-earth-vector/master/geojson/ne_110m_admin_0_countries.geojson";

fn main() {
    // Only rerun this script when build.rs itself changes.
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/ne_110m_countries.json");

    let assets_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap()).join("assets");
    std::fs::create_dir_all(&assets_dir).ok();

    let dest = assets_dir.join("ne_110m_countries.json");
    if dest.exists() {
        return; // already downloaded — nothing to do
    }

    eprintln!("cargo:warning=Downloading Natural Earth 110m country outlines for bbox map…");

    // Try curl first (available on macOS and most Linux distros by default).
    let ok = Command::new("curl")
        .args([
            "-fsSL",
            "--max-time",
            "30",
            "-o",
            dest.to_str().unwrap(),
            NE_URL,
        ])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if ok {
        eprintln!("cargo:warning=Downloaded ne_110m_countries.json successfully.");
        return;
    }

    // Fall back to wget.
    let ok = Command::new("wget")
        .args(["-q", "--timeout=30", "-O", dest.to_str().unwrap(), NE_URL])
        .status()
        .map(|s| s.success())
        .unwrap_or(false);

    if ok {
        eprintln!("cargo:warning=Downloaded ne_110m_countries.json via wget.");
        return;
    }

    // Neither curl nor wget available (or network is offline).  The bbox map
    // will fall back to the graticule-only display.  This is not a build error.
    eprintln!(
        "cargo:warning=Could not download ne_110m_countries.json (no curl/wget or offline). \
         The bbox map will show graticule only. Drop the file manually at {dest:?} to enable coastlines."
    );
}
