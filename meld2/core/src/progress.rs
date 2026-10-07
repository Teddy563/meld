//! Arnis `--progress json` (NDJSON v1). Arnis prints its usual human lines
//! around the records, so only lines starting `{"v":1,` are records.

use serde::Deserialize;

#[derive(Debug, Deserialize, PartialEq)]
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
        cpu_s: Option<f64>,
        peak_rss_mb: Option<u64>,
        chunks: u64,
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
