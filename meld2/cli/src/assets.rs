//! Images the page shows, built into the binary so the app and `serve` need
//! no files beside them: Meld 1's wordmark (`hero-*`, from web/) and the
//! option previews of Arnis at Scale's settings (Apache-2.0, louis-e/arnis
//! `src/gui/images/previews`, branch arnis-scale-phase-6).

macro_rules! files {
    ($($n:literal),* $(,)?) => {
        &[$(($n, include_bytes!(concat!("../assets/", $n)) as &[u8])),*]
    };
}

const FILES: &[(&str, &[u8])] = files![
    "cave-style-all-mix-more-ores.webp",
    "cave-style-all-mix.webp",
    "cave-style-more-mix-more-ores.webp",
    "cave-style-more-mix.webp",
    "cave-style-more-vanilla-more-ores.webp",
    "cave-style-more-vanilla.webp",
    "cave-style-vanilla-more-ores.webp",
    "cave-style-vanilla.webp",
    "climate-mode-origin.webp",
    "climate-mode-per-position.webp",
    "field-mix-classic.webp",
    "field-mix-pasture.webp",
    "field-mix-patchwork.webp",
    "field-mix-prairie.webp",
    "field-mix-smallholding.webp",
    "grass-mix-pasture.webp",
    "grass-mix-patchwork.webp",
    "grass-mix-prairie.webp",
    "grass-mix-smallholding.webp",
    "grass-texture-off.webp",
    "grass-texture-on.webp",
    "hero-arnis-worlds.webp",
    "hero-d.webp",
    "hero-e.webp",
    "hero-l.webp",
    "hero-m.webp",
    "hero-title-full.webp",
    "land-mix-pasture.webp",
    "land-mix-prairie.webp",
    "land-mix-smallholding.webp",
    "land-texture-off.webp",
    "land-texture-on.webp",
    "river-bed-off.webp",
    "river-bed-v1.webp",
    "road-detail-clean.webp",
    "road-detail-compact.webp",
    "road-detail-max.webp",
    "scatter-both.webp",
    "scatter-bushes.webp",
    "scatter-off.webp",
    "scatter-rocks.webp",
    "snow-mode-manual.webp",
    "snow-mode-off.webp",
    "snow-mode-peaks.webp",
    "snow-mode-realistic.webp",
    "tree-realm-afr.webp",
    "tree-realm-asn.webp",
    "tree-realm-aus.webp",
    "tree-realm-auto.webp",
    "tree-realm-ena.webp",
    "tree-realm-eur.webp",
    "tree-realm-fl.webp",
    "tree-realm-ind.webp",
    "tree-realm-sam.webp",
    "tree-realm-vanilla-plus.webp",
    "tree-realm-wna.webp",
    "tree-size-big.webp",
    "tree-size-giant.webp",
    "tree-size-medium.webp",
    "tree-size-small.webp",
    "tree-size-tall.webp",
    "water-detail-default.webp",
    "water-detail-scaled.webp",
    // Leaflet 1.9.4 (BSD-2) and Leaflet.draw 1.0.4 (MIT), so the map works offline.
    "leaflet/leaflet.js",
    "leaflet/leaflet.css",
    "leaflet/leaflet.draw.js",
    "leaflet/leaflet.draw.css",
    "leaflet/images/layers.png",
    "leaflet/images/layers-2x.png",
    "leaflet/images/marker-icon.png",
    "leaflet/images/marker-icon-2x.png",
    "leaflet/images/marker-shadow.png",
    "leaflet/images/spritesheet.png",
    "leaflet/images/spritesheet-2x.png",
    "leaflet/images/spritesheet.svg",
];

/// The file named `name`, if the page has one by that name, with its type.
pub fn get(name: &str) -> Option<(&'static [u8], &'static str)> {
    let (_, bytes) = FILES.iter().find(|(n, _)| *n == name)?;
    let kind = match name.rsplit('.').next() {
        Some("js") => "text/javascript",
        Some("css") => "text/css",
        Some("png") => "image/png",
        Some("svg") => "image/svg+xml",
        _ => "image/webp",
    };
    Some((bytes, kind))
}
