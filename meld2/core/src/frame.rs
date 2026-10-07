//! A One World's frame: Web Mercator with block (0, 0) at the world's
//! origin, the same math as Arnis's `projection/web_mercator.rs`, and the
//! parts of `arnis_one_world.json` Meld reads (origin, scale, areas).

use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::Path;

/// Arnis's spherical Earth (WGS84 mean radius).
const EARTH_RADIUS: f64 = 6_371_000.0;

/// Block (0, 0) at `origin_lat, origin_lon`; x grows east, z grows south;
/// a block is `1 / scale` metres at the origin latitude.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Frame {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub scale: f64,
}

fn mercator_y(lat: f64) -> f64 {
    EARTH_RADIUS
        * (std::f64::consts::FRAC_PI_4 + lat.to_radians() / 2.0)
            .tan()
            .ln()
}

impl Frame {
    /// A new world's frame as Arnis stores it: the origin rounded to 7 decimals.
    pub fn new(origin_lat: f64, origin_lon: f64, scale: f64) -> Self {
        let r = |v: f64| (v * 1e7).round() / 1e7;
        Self {
            origin_lat: r(origin_lat),
            origin_lon: r(origin_lon),
            scale,
        }
    }

    fn k(&self) -> f64 {
        self.scale * self.origin_lat.to_radians().cos()
    }

    pub fn x(&self, lon: f64) -> f64 {
        EARTH_RADIUS * (lon - self.origin_lon).to_radians() * self.k()
    }

    pub fn z(&self, lat: f64) -> f64 {
        -(mercator_y(lat) - mercator_y(self.origin_lat)) * self.k()
    }

    pub fn lon(&self, x: f64) -> f64 {
        self.origin_lon + (x / (EARTH_RADIUS * self.k())).to_degrees()
    }

    pub fn lat(&self, z: f64) -> f64 {
        let y = mercator_y(self.origin_lat) - z / self.k();
        (2.0 * ((y / EARTH_RADIUS).exp().atan() - std::f64::consts::FRAC_PI_4)).to_degrees()
    }

    /// The `--origin` value that gives a new world this frame.
    pub fn origin_arg(&self) -> String {
        format!("{},{}", self.origin_lat, self.origin_lon)
    }
}

/// What Meld reads of a world's `arnis_one_world.json`.
#[derive(Debug, Deserialize)]
pub struct Manifest {
    pub origin_lat: f64,
    pub origin_lon: f64,
    pub scale: f64,
    #[serde(default)]
    pub disable_height_limit: bool,
    #[serde(default)]
    pub areas: Vec<Area>,
}

/// One area the world holds, as inclusive block bounds.
#[derive(Debug, Deserialize)]
pub struct Area {
    pub min_x: i64,
    pub min_z: i64,
    pub max_x: i64,
    pub max_z: i64,
}

pub const MANIFEST: &str = "arnis_one_world.json";

impl Manifest {
    /// The world's manifest, or `None` when the world does not exist yet.
    pub fn load(world_dir: &Path) -> Result<Option<Self>> {
        let file = world_dir.join(MANIFEST);
        match std::fs::read_to_string(&file) {
            Ok(text) => serde_json::from_str(&text)
                .map(Some)
                .with_context(|| format!("reading {}", file.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(e).with_context(|| format!("reading {}", file.display())),
        }
    }

    pub fn frame(&self) -> Frame {
        Frame {
            origin_lat: self.origin_lat,
            origin_lon: self.origin_lon,
            scale: self.scale,
        }
    }

    /// The bounds of every area, `[min_x, min_z, max_x, max_z]` inclusive:
    /// what Arnis's `--world-border` surrounds.
    pub fn extent(&self) -> Option<[i64; 4]> {
        let first = self.areas.first()?;
        Some(self.areas.iter().fold(
            [first.min_x, first.min_z, first.max_x, first.max_z],
            |e, a| {
                [
                    e[0].min(a.min_x),
                    e[1].min(a.min_z),
                    e[2].max(a.max_x),
                    e[3].max(a.max_z),
                ]
            },
        ))
    }

    /// The world's build height, for regions that span it.
    pub fn y_range(&self) -> (i32, i32) {
        if self.disable_height_limit {
            (-2032, 2031)
        } else {
            (-64, 319)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Arnis's own checks (web_mercator.rs tests): the origin is block 0,
    /// one metre is one block at the origin, north is -z, and it round-trips.
    #[test]
    fn matches_arnis_web_mercator() {
        let f = Frame::new(48.8566, 2.3522, 1.0);
        assert!(f.x(2.3522).abs() < 1e-9 && f.z(48.8566).abs() < 1e-9);
        let d = 0.0005_f64;
        let ground = EARTH_RADIUS * (2.0 * d).to_radians();
        let ratio = (f.z(48.8566 - d) - f.z(48.8566 + d)) / ground;
        assert!((ratio - 1.0).abs() < 1e-3, "{ratio}");
        assert!(f.z(49.0) < 0.0 && f.x(3.0) > 0.0);
        for (lat, lon) in [(47.14, 9.52), (-33.9, 151.2), (64.1, -21.9)] {
            let (x, z) = (f.x(lon), f.z(lat));
            assert!((f.lat(z) - lat).abs() < 1e-9 && (f.lon(x) - lon).abs() < 1e-9);
        }
        assert_eq!(Frame::new(47.123456789, 9.5, 2.0).origin_lat, 47.1234568);
    }
}
