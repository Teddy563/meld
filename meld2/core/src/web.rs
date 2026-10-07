//! The two web lookups Meld makes for itself: place search (Nominatim, as
//! Meld 1 and Arnis's map do) and the update check (GitHub releases of
//! Teddy563/meld; it only tells, it installs nothing).

use anyhow::{Context, Result};
use serde::Serialize;
use serde_json::Value;
use std::path::Path;
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub const VERSION: &str = env!("CARGO_PKG_VERSION");
/// Nominatim's policy asks for an identifying User-Agent.
pub const USER_AGENT: &str = concat!(
    "Meld/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/Teddy563/meld)"
);
const NOMINATIM: &str = "https://nominatim.openstreetmap.org/search";
const RELEASES: &str = "https://api.github.com/repos/Teddy563/meld/releases?per_page=30";
/// A good update answer is fresh for a day, a failed one for an hour (Meld 1's `update.py`).
const TTL_OK: u64 = 24 * 3600;
const TTL_FAIL: u64 = 3600;

fn get(url: &str, query: &[(&str, &str)]) -> Result<Value> {
    let mut req = ureq::get(url)
        .header("User-Agent", USER_AGENT)
        .config()
        .timeout_global(Some(Duration::from_secs(15)))
        .build();
    for (k, v) in query {
        req = req.query(*k, *v);
    }
    let text = req
        .call()
        .with_context(|| format!("asking {url}"))?
        .into_body()
        .read_to_string()?;
    Ok(serde_json::from_str(&text)?)
}

#[derive(Debug, Serialize, PartialEq)]
pub struct Place {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    /// south, west, north, east: the order of a selection's bbox.
    pub bbox: Option<[f64; 4]>,
    /// The outline, as GeoJSON, when asked for (a country or city to select).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub geojson: Option<Value>,
}

/// One request per second across the process, Nominatim's usage policy.
static LAST: Mutex<Option<Instant>> = Mutex::new(None);

/// Up to five places for `q`, at most one request a second.
pub fn search(q: &str, outline: bool) -> Result<Vec<Place>> {
    {
        let mut last = LAST.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(t) = *last {
            if let Some(wait) = Duration::from_secs(1).checked_sub(t.elapsed()) {
                std::thread::sleep(wait);
            }
        }
        *last = Some(Instant::now());
    }
    let mut query = vec![
        ("q", q),
        ("format", "jsonv2"),
        ("limit", "5"),
        ("accept-language", "en"),
    ];
    if outline {
        query.push(("polygon_geojson", "1"));
    }
    Ok(places(&get(NOMINATIM, &query)?))
}

/// Nominatim's answer as places (its numbers are strings; its bbox is s, n, w, e).
pub fn places(v: &Value) -> Vec<Place> {
    let num = |x: &Value| x.as_str().and_then(|s| s.parse().ok()).or(x.as_f64());
    v.as_array()
        .into_iter()
        .flatten()
        .filter_map(|p| {
            let b: Vec<f64> = p["boundingbox"]
                .as_array()
                .map(|a| a.iter().filter_map(num).collect())
                .unwrap_or_default();
            Some(Place {
                name: p["display_name"].as_str()?.into(),
                lat: num(&p["lat"])?,
                lon: num(&p["lon"])?,
                bbox: (b.len() == 4).then(|| [b[0], b[2], b[1], b[3]]),
                geojson: p.get("geojson").cloned(),
            })
        })
        .collect()
}

#[derive(Debug, Default, Serialize, serde::Deserialize, Clone, PartialEq)]
pub struct Update {
    pub current: String,
    /// The newest release's version, when it is newer than this build.
    pub newer: Option<String>,
    pub url: Option<String>,
    /// Unix seconds of the check.
    pub checked: u64,
    pub error: Option<String>,
}

/// The newest release in `releases` (GitHub's list) above `current`. Drafts
/// never count; pre-releases count only while `current` is one.
pub fn newest(releases: &Value, current: &str) -> Option<(String, String)> {
    let cur = semver::Version::parse(current).ok()?;
    releases
        .as_array()?
        .iter()
        .filter(|r| r["draft"] != true && (r["prerelease"] != true || !cur.pre.is_empty()))
        .filter_map(|r| {
            let tag = r["tag_name"].as_str()?;
            let v = semver::Version::parse(tag.trim_start_matches(|c: char| !c.is_ascii_digit()))
                .ok()?;
            Some((v, r["html_url"].as_str().unwrap_or_default().to_string()))
        })
        .filter(|(v, _)| *v > cur)
        .max_by(|a, b| a.0.cmp(&b.0))
        .map(|(v, url)| (v.to_string(), url))
}

/// Checks GitHub at most once a day (an hour after a failure), remembering
/// the answer in `<data>/update.json`. `force` asks now.
pub fn check_update(data: &Path, force: bool) -> Update {
    let file = data.join("update.json");
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs());
    let cached: Option<Update> = std::fs::read(&file)
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok());
    if let Some(c) = cached.filter(|c| c.current == VERSION && !force) {
        let ttl = if c.error.is_some() { TTL_FAIL } else { TTL_OK };
        if now.saturating_sub(c.checked) < ttl {
            return c;
        }
    }
    let mut u = Update {
        current: VERSION.into(),
        checked: now,
        ..Default::default()
    };
    match get(RELEASES, &[]) {
        Ok(list) => {
            if let Some((v, url)) = newest(&list, VERSION) {
                u.newer = Some(v);
                u.url = Some(url);
            }
        }
        Err(e) => u.error = Some(format!("{e:#}")),
    }
    let _ = std::fs::create_dir_all(data);
    let _ = std::fs::write(&file, serde_json::to_vec_pretty(&u).unwrap_or_default());
    u
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn nominatim_answer_to_places() {
        let v = json!([{"display_name": "Vaduz, Liechtenstein", "lat": "47.14", "lon": "9.52",
                        "boundingbox": ["47.09", "47.17", "9.47", "9.56"]},
                       {"lat": "1", "lon": "2"}]);
        assert_eq!(
            places(&v),
            [Place {
                name: "Vaduz, Liechtenstein".into(),
                lat: 47.14,
                lon: 9.52,
                bbox: Some([47.09, 9.47, 47.17, 9.56]),
                geojson: None,
            }]
        );
    }

    #[test]
    fn newest_release_above_this_build() {
        let r = |tag: &str, pre: bool| json!({"tag_name": tag, "prerelease": pre, "draft": false, "html_url": tag});
        let list = json!([
            r("v1.9.9", false),
            r("v2.0.0-beta.1", true),
            r("v2.0.0-beta.2", true)
        ]);
        assert_eq!(newest(&list, "2.0.0-beta.1").unwrap().0, "2.0.0-beta.2");
        // A stable build is not offered pre-releases, nor older releases.
        assert_eq!(newest(&list, "2.0.0"), None);
        let list = json!([
            r("v2.0.0", false),
            r("v2.1.0", false),
            json!({"tag_name": "v9.0.0", "draft": true})
        ]);
        assert_eq!(newest(&list, "2.0.0").unwrap().0, "2.1.0");
        assert_eq!(newest(&list, "2.0.0-beta.1").unwrap().0, "2.1.0");
    }
}
