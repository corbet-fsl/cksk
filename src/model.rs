// Thin consumer of the Gatus statuses API. NO derived metrics computed here (no uptime math):
// the frontend renders only what the API returns. Anything derived is the observability API's job.

use serde::Deserialize;

#[derive(Debug, Clone, Deserialize)]
pub struct Endpoint {
    pub name: String,
    pub group: String,
    #[allow(dead_code)]
    pub key: String,
    #[serde(default)]
    pub results: Vec<GResult>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GResult {
    #[serde(default)]
    pub success: bool,
    /// Response time in NANOSECONDS (Gatus). Absent/0 for push endpoints.
    #[serde(default)]
    pub duration: u64,
    /// HTTP status code — present only for active-probe endpoints, absent for push/external ones.
    #[serde(default)]
    pub status: Option<u32>,
}

impl Endpoint {
    pub fn latest(&self) -> Option<&GResult> {
        self.results.last()
    }
    pub fn up(&self) -> Option<bool> {
        self.latest().map(|r| r.success)
    }
    /// Latency of the latest result in ms, but only for active probes (status present).
    pub fn latency_ms(&self) -> Option<f64> {
        let r = self.latest()?;
        if r.status.is_some() {
            Some(r.duration as f64 / 1_000_000.0)
        } else {
            None // push/external endpoint: no meaningful latency to render
        }
    }
    /// The raw recent success/fail history (oldest..newest) — rendered as a strip, not computed on.
    pub fn history(&self, max: usize) -> Vec<bool> {
        let n = self.results.len();
        let start = n.saturating_sub(max);
        self.results[start..].iter().map(|r| r.success).collect()
    }
}

/// Load from a local file path (dev/verify) or an https URL (live). Source is a path if it exists.
pub fn load(source: &str) -> Result<Vec<Endpoint>, String> {
    let bytes = if std::path::Path::new(source).exists() {
        std::fs::read(source).map_err(|e| format!("read {source}: {e}"))?
    } else {
        let resp = ureq::get(source)
            .timeout(std::time::Duration::from_secs(10))
            .call()
            .map_err(|e| format!("GET {source}: {e}"))?;
        let mut buf = Vec::new();
        use std::io::Read;
        resp.into_reader()
            .read_to_end(&mut buf)
            .map_err(|e| format!("read body: {e}"))?;
        buf
    };
    serde_json::from_slice(&bytes).map_err(|e| format!("parse json: {e}"))
}

/// Groups in first-seen order, each with the indices of its endpoints.
pub fn grouped(endpoints: &[Endpoint]) -> Vec<(String, Vec<usize>)> {
    let mut out: Vec<(String, Vec<usize>)> = Vec::new();
    for (i, e) in endpoints.iter().enumerate() {
        if let Some(g) = out.iter_mut().find(|(name, _)| *name == e.group) {
            g.1.push(i);
        } else {
            out.push((e.group.clone(), vec![i]));
        }
    }
    out
}
