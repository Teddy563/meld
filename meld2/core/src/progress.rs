//! Arnis `--progress json` (NDJSON v1). Arnis prints its usual human lines
//! around the records, so only lines starting `{"v":1,` are records.

use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize, Serialize, PartialEq)]
#[serde(tag = "type", rename_all = "lowercase")]
pub enum Event {
    /// A new status message; `progress` is null for one that leaves the bar alone.
    Phase {
        name: String,
        progress: Option<f64>,
    },
    /// The job's percentage, monotonic.
    Progress {
        progress: f64,
    },
    /// A piece of a `--unit-regions` job: start, retry, done, skipped or failed.
    Piece {
        piece: u32,
        of: u32,
        state: String,
        wall_s: Option<f64>,
        peak_rss_mb: Option<u64>,
    },
    /// Download or bake of an `.osm.pbf` extract.
    Transfer {
        stage: String,
        name: String,
        done_bytes: u64,
        total_bytes: u64,
        percent: f64,
    },
    Error {
        message: String,
    },
    /// Closes a successful run.
    Done {
        wall_s: f64,
        /// Arnis's own process; Meld puts the whole process tree's here
        /// where it can measure it (Windows: the run's Job Object).
        cpu_s: Option<f64>,
        peak_rss_mb: Option<u64>,
        chunks: u64,
        /// Not Arnis's: the memory the whole process tree had committed at
        /// its peak, from the Job Object (Windows).
        #[serde(default, skip_serializing_if = "Option::is_none")]
        tree_peak_mb: Option<u64>,
    },
    /// A record type this Meld does not know; later v1 Arnis may add some.
    #[serde(other)]
    Other,
}

/// The record on `line`, if it is one this protocol version reads.
pub fn parse(line: &str) -> Option<Event> {
    if !line.starts_with(r#"{"v":1,"#) {
        return None;
    }
    serde_json::from_str(line).ok()
}

/// Why Arnis stopped, in its own words: the last `Error: ` line of what it
/// printed (colour codes taken out). Its `--progress json` error record only
/// says "generation failed, see stderr".
/// Its "  - " list below (what is missing) joins it, the first few.
pub fn last_error(output: &str) -> Option<String> {
    let lines: Vec<String> = output.lines().map(plain).collect();
    let (i, msg) = lines.iter().enumerate().rev().find_map(|(i, l)| {
        let msg = l.trim().strip_prefix("Error:")?.trim();
        (!msg.is_empty()).then_some((i, msg))
    })?;
    const MORE: usize = 6;
    let more: Vec<&str> = lines[i + 1..]
        .iter()
        .map_while(|l| l.strip_prefix("  - "))
        .collect();
    let mut out = msg.to_string();
    if !more.is_empty() {
        out = format!("{} {}", out, more[..more.len().min(MORE)].join("; "));
        if more.len() > MORE {
            out.push_str(&format!("; and {} more", more.len() - MORE));
        }
    }
    Some(out)
}

/// `line` without its colour codes (ESC [ parameters, up to the final letter).
fn plain(line: &str) -> String {
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c == '\u{1b}' {
            chars.by_ref().find(|c| c.is_ascii_alphabetic());
        } else {
            out.push(c);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn events(fixture: &str) -> Vec<Event> {
        fixture.lines().filter_map(parse).collect()
    }

    #[test]
    fn piece_job_recorded_from_arnis_3_4_0_beta_1() {
        let ev = events(include_str!("../tests/fixtures/pieces.stdout"));
        let pieces: Vec<_> = ev
            .iter()
            .filter_map(|e| match e {
                Event::Piece {
                    piece, of, state, ..
                } => Some((*piece, *of, state.as_str())),
                _ => None,
            })
            .collect();
        assert_eq!(pieces.len(), 8);
        assert_eq!(pieces[0], (0, 4, "start"));
        assert_eq!(pieces[7], (3, 4, "done"));
        assert!(matches!(ev.last(), Some(Event::Done { chunks: 224, .. })));
        // Percentages never go back.
        let pct: Vec<f64> = ev
            .iter()
            .filter_map(|e| match e {
                Event::Progress { progress } => Some(*progress),
                _ => None,
            })
            .collect();
        assert!(pct.len() > 100 && pct.windows(2).all(|w| w[0] <= w[1]));
    }

    #[test]
    fn transfer_records() {
        let ev = events(include_str!("../tests/fixtures/transfer.stdout"));
        let t = ev
            .iter()
            .filter(|e| matches!(e, Event::Transfer { .. }))
            .count();
        assert_eq!(t, 8);
        assert!(ev.contains(&Event::Transfer {
            stage: "download".into(),
            name: "liechtenstein".into(),
            done_bytes: 3463632,
            total_bytes: 3463632,
            percent: 40.0,
        }));
    }

    #[test]
    fn error_run_has_error_and_no_done() {
        let ev = events(include_str!("../tests/fixtures/offline-error.stdout"));
        assert!(ev.contains(&Event::Error {
            message: "generation failed, see stderr".into()
        }));
        assert!(!ev.iter().any(|e| matches!(e, Event::Done { .. })));
        assert!(ev.contains(&Event::Phase {
            name: "Downloading data...".into(),
            progress: Some(1.0)
        }));
    }

    /// The user's `buc` log: two stopped runs, then Arnis refusing the One World.
    #[test]
    fn last_error_is_arnis_reason() {
        let log = include_str!("../tests/fixtures/one-world-mismatch.log");
        let e = last_error(log).unwrap();
        assert!(
            e.starts_with("This area cannot be added to the One World at ")
                && e.ends_with(": ground level -62 does not match the world's 0. Change the setting, or use another world name to start a new One World."),
            "{e}"
        );
        let coloured = "x\n\u{1b}[1;31mError:\u{1b}[0m no bbox\n  piece 3/4 done\n";
        assert_eq!(last_error(coloured).as_deref(), Some("no bbox"));
        let listed =
            "Error: the cache lacks:\n  - OSM data\n  - land cover\nRun it with --prewarm.\n";
        assert_eq!(
            last_error(listed).as_deref(),
            Some("the cache lacks: OSM data; land cover")
        );
        // The stopped runs before the refusals printed no error.
        assert_eq!(last_error(&log[..log.find("Error:").unwrap()]), None);
    }

    #[test]
    fn other_lines_and_versions_are_skipped() {
        assert_eq!(parse("One World: area #1 recorded."), None);
        assert_eq!(parse(r#"{"v":2,"type":"progress","progress":1.0}"#), None);
        assert_eq!(
            parse(r#"{"v":1,"type":"plan","units":[]}"#),
            Some(Event::Other)
        );
    }
}
