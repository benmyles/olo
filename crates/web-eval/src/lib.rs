//! Compare paired browser observations without treating missing evidence
//! as a pass. No browser is launched, and nothing reaches a real profile.
#![forbid(unsafe_code)]

use std::{
    collections::{BTreeMap, BTreeSet},
    fs::File,
    io::Read,
    path::Path,
};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

mod pixels;
pub use pixels::{PixelDifference, compare_png};

pub type Result<T> = std::result::Result<T, String>;
pub const CHECKPOINTS: [&str; 4] = ["loaded", "interaction", "scrolled", "history"];
const MAX_JSON: u64 = 2 * 1024 * 1024;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Selection {
    pub kind: String,
    pub description: String,
    pub ranking_url: String,
    pub ranking_sha256: String,
    pub ranking_date: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Viewport {
    pub id: String,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Matrix {
    pub viewports: Vec<Viewport>,
    pub scales: Vec<u32>,
    pub themes: Vec<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Check {
    pub id: String,
    pub description: String,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Site {
    pub id: String,
    pub origin: String,
    pub domain: String,
    pub rank: u32,
    pub category: String,
    pub journey: String,
    pub checks: Vec<Check>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Suite {
    pub schema_version: u32,
    pub selection: Selection,
    pub matrix: Matrix,
    pub sites: Vec<Site>,
}

fn safe_id(id: &str) -> bool {
    !id.is_empty() && id.len() <= 100 && id.bytes().all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || matches!(b, b'-' | b'_'))
}

fn digest(s: &str) -> bool {
    s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit())
}
fn require(condition: bool, reason: &str) -> Result<()> {
    if condition { Ok(()) } else { Err(reason.into()) }
}

impl Suite {
    pub fn validate(&self) -> Result<()> {
        require(self.schema_version == 1, "unsupported suite schema")?;
        require(self.sites.len() == 100, "suite must contain exactly 100 sites")?;
        require(digest(&self.selection.ranking_sha256), "ranking source SHA-256 is required")?;
        require(self.selection.ranking_url.starts_with("https://"), "ranking source URL is required")?;
        require(!self.selection.kind.is_empty() && !self.selection.description.is_empty() && !self.selection.ranking_date.is_empty(), "selection provenance is incomplete")?;
        require(self.matrix.scales == [100, 125, 150, 175, 200], "matrix must include all five scales, in order")?;
        require(self.matrix.themes == ["light", "dark", "night"], "matrix must include light, dark and night, in order")?;
        require(self.matrix.viewports.len() == 2, "matrix must include desktop and narrow viewports")?;
        let mut ids = BTreeSet::new();
        let mut domains = BTreeSet::new();
        for v in &self.matrix.viewports {
            require(safe_id(&v.id) && ids.insert(v.id.as_str()), "invalid or duplicate viewport ID")?;
            require((320..=2048).contains(&v.width) && (320..=2048).contains(&v.height), "viewport outside capture limits")?;
        }
        require(self.matrix.viewports.iter().any(|v| v.width == 1280 && v.height == 800), "1280x800 desktop viewport is required")?;
        require(self.matrix.viewports.iter().any(|v| v.width <= 480), "narrow viewport is required")?;
        ids.clear();
        for s in &self.sites {
            require(safe_id(&s.id) && ids.insert(s.id.as_str()), "invalid or duplicate site ID")?;
            require(!s.domain.is_empty() && s.domain == s.domain.to_ascii_lowercase() && domains.insert(s.domain.as_str()), "invalid or duplicate domain")?;
            require(s.origin == format!("https://{}/", s.domain), "site origin must be the domain's HTTPS root")?;
            require(s.rank > 0 && !s.category.is_empty() && !s.journey.is_empty(), "site selection or journey is incomplete")?;
            require(s.checks.len() >= 5 && s.checks.len() <= 32, "journey needs 5 to 32 explicit assertions")?;
            let mut checks = BTreeSet::new();
            for c in &s.checks {
                require(safe_id(&c.id) && checks.insert(c.id.as_str()) && !c.description.is_empty(), "invalid or duplicate assertion")?;
            }
            for required in ["document_ready", "meaningful_content", "scroll_changed", "navigation_changed", "history_restored"] {
                require(checks.contains(required), "journey lacks a mandatory assertion")?;
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct RunMetadata {
    pub schema_version: u32,
    pub engine: String,
    pub version: String,
    pub binary_sha256: String,
    pub source_revision: String,
    pub suite_sha256: String,
    pub resource_archive_sha256: String,
    pub fonts_sha256: String,
    pub machine: String,
    pub os: String,
    pub locale: String,
    pub timezone: String,
    pub captured_at: String,
    pub release: bool,
}

impl RunMetadata {
    fn validate(&self, engine: &str, suite_sha256: &str) -> Result<()> {
        require(self.schema_version == 1 && self.engine == engine && self.release, "wrong engine, schema or non-release capture")?;
        require(self.suite_sha256 == suite_sha256, "capture belongs to a different suite")?;
        for value in [&self.binary_sha256, &self.resource_archive_sha256, &self.fonts_sha256] {
            require(digest(value), "capture requires binary/resource/font hashes")?;
        }
        require(
            !self.version.is_empty()
                && !self.source_revision.is_empty()
                && !self.machine.is_empty()
                && !self.os.is_empty()
                && !self.locale.is_empty()
                && !self.timezone.is_empty()
                && !self.captured_at.is_empty(),
            "capture metadata is incomplete",
        )
    }
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

impl Rect {
    fn valid(&self) -> bool {
        [self.x, self.y, self.width, self.height].iter().all(|v| v.is_finite()) && self.width > 0.0 && self.height > 0.0
    }
    fn matches(&self, other: &Self) -> bool {
        self.valid() && other.valid() && [(self.x, other.x), (self.y, other.y), (self.width, other.width), (self.height, other.height)].iter().all(|(a, b)| (a - b).abs() <= 0.5)
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Checkpoint {
    pub text: String,
    pub document_url: String,
    pub document_generation: u64,
    pub scroll: [f64; 2],
    pub landmarks: BTreeMap<String, Rect>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Cost {
    pub navigation_ms: f64,
    pub first_input_ms: f64,
    pub worst_frame_ms: f64,
    pub peak_rss_kib: u64,
    pub idle_duration_ms: u64,
    pub idle_frames: u64,
    pub idle_cpu_ms: f64,
    pub steady_rss_growth_kib: u64,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub schema_version: u32,
    pub site: String,
    pub viewport: String,
    pub scale: u32,
    pub theme: String,
    pub status: String,
    pub blocked_reason: Option<String>,
    pub assertions: BTreeMap<String, bool>,
    pub checkpoints: BTreeMap<String, Checkpoint>,
    pub cost: Cost,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    Passed,
    Failed,
    Blocked,
    Missing,
}

#[derive(Debug, Serialize)]
pub struct CaseResult {
    pub site: String,
    pub viewport: String,
    pub scale: u32,
    pub theme: String,
    pub outcome: Outcome,
    pub reasons: Vec<String>,
    pub pixels: BTreeMap<String, PixelDifference>,
}

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub scope: String,
    pub suite_sha256: String,
    pub reference: Option<RunMetadata>,
    pub candidate: Option<RunMetadata>,
    pub metadata_errors: Vec<String>,
    pub total: usize,
    pub passed: usize,
    pub failed: usize,
    pub blocked: usize,
    pub missing: usize,
    pub sites_total: usize,
    pub sites_passed: usize,
    pub cases: Vec<CaseResult>,
}

fn read_bytes(path: &Path) -> Result<Vec<u8>> {
    let mut bytes = Vec::new();
    File::open(path).map_err(|e| format!("{}: {e}", path.display()))?.take(MAX_JSON + 1).read_to_end(&mut bytes).map_err(|e| e.to_string())?;
    require(bytes.len() as u64 <= MAX_JSON, "JSON artifact exceeds byte limit")?;
    Ok(bytes)
}

fn read_json<T: serde::de::DeserializeOwned>(path: &Path) -> Result<T> {
    let bytes = read_bytes(path)?;
    serde_json::from_slice(&bytes).map_err(|e| format!("{}: {e}", path.display()))
}

fn words(text: &str) -> String {
    text.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn compare_observations(site: &Site, reference: &Observation, candidate: &Observation, reasons: &mut Vec<String>) {
    for (who, o) in [("reference", reference), ("candidate", candidate)] {
        for check in &site.checks {
            if o.assertions.get(&check.id) != Some(&true) {
                reasons.push(format!("{who}: assertion {} absent or failed", check.id));
            }
        }
        if o.checkpoints.len() != CHECKPOINTS.len() {
            reasons.push(format!("{who}: checkpoint set incomplete or unexpected"));
        }
        for id in CHECKPOINTS {
            let Some(c) = o.checkpoints.get(id) else {
                reasons.push(format!("{who}: missing checkpoint {id}"));
                continue;
            };
            if words(&c.text).chars().count() < 40
                || c.landmarks.len() < 3
                || c.landmarks.len() > 2048
                || c.landmarks.values().any(|r| !r.valid())
                || !c.document_url.starts_with("https://")
                || c.document_generation == 0
                || c.scroll.iter().any(|v| !v.is_finite() || *v < 0.0)
            {
                reasons.push(format!("{who}: {id} lacks meaningful content/geometry"));
            }
        }
        if let (Some(loaded), Some(interaction), Some(scrolled), Some(history)) =
            (o.checkpoints.get("loaded"), o.checkpoints.get("interaction"), o.checkpoints.get("scrolled"), o.checkpoints.get("history"))
        {
            if loaded.document_url == interaction.document_url
                || loaded.document_generation == interaction.document_generation
                || interaction.document_url != scrolled.document_url
                || interaction.document_generation != scrolled.document_generation
            {
                reasons.push(format!("{who}: navigation checkpoints did not change/retain the expected document"));
            }
            if interaction.scroll.iter().zip(scrolled.scroll).all(|(a, b)| (a - b).abs() <= 1.0) {
                reasons.push(format!("{who}: scroll checkpoints did not move"));
            }
            if loaded.document_url != history.document_url || words(&loaded.text) != words(&history.text) || loaded.scroll.iter().zip(history.scroll).any(|(a, b)| (a - b).abs() > 0.5) {
                reasons.push(format!("{who}: history did not restore document, content and position"));
            }
        }
        let c = &o.cost;
        if [c.navigation_ms, c.first_input_ms, c.worst_frame_ms, c.idle_cpu_ms].iter().any(|v| !v.is_finite() || *v < 0.0) || c.peak_rss_kib == 0 || c.idle_duration_ms < 1000 {
            reasons.push(format!("{who}: invalid cost evidence"));
        }
    }
    for id in CHECKPOINTS {
        if let (Some(r), Some(c)) = (reference.checkpoints.get(id), candidate.checkpoints.get(id)) {
            if words(&r.text) != words(&c.text) || r.document_url != c.document_url || r.scroll.iter().zip(c.scroll).any(|(a, b)| (a - b).abs() > 0.5) {
                reasons.push(format!("{id}: document content/URL differs"));
            }
            if r.landmarks.len() != c.landmarks.len() || r.landmarks.iter().any(|(key, rect)| c.landmarks.get(key).is_none_or(|c| !rect.matches(c))) {
                reasons.push(format!("{id}: landmark geometry differs"));
            }
        }
    }
    let c = &candidate.cost;
    let r = &reference.cost;
    if c.first_input_ms > 8.3 || c.worst_frame_ms > 8.3 {
        reasons.push("candidate exceeds 8.3 ms input/frame budget".into());
    }
    if c.navigation_ms > 5000.0 || c.navigation_ms > r.navigation_ms * 1.1 + 1.0 {
        reasons.push("candidate exceeds navigation budget".into());
    }
    if c.peak_rss_kib > r.peak_rss_kib || c.steady_rss_growth_kib > 1024 {
        reasons.push("candidate exceeds memory/growth budget".into());
    }
    if c.idle_frames != 0 || c.idle_cpu_ms > c.idle_duration_ms as f64 * 0.001 {
        reasons.push("candidate does work while idle".into());
    }
}

pub fn evaluate(suite_path: &Path, reference_root: &Path, candidate_root: &Path) -> Result<Report> {
    let bytes = read_bytes(suite_path)?;
    let suite: Suite = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    suite.validate()?;
    let suite_sha256 = format!("{:x}", Sha256::digest(&bytes));
    let mut metadata_errors = Vec::new();
    let mut metadata = |root: &Path, engine| match read_json::<RunMetadata>(&root.join("run.json")) {
        Ok(run) => {
            if let Err(e) = run.validate(engine, &suite_sha256) {
                metadata_errors.push(format!("{engine}: {e}"));
            }
            Some(run)
        }
        Err(e) => {
            metadata_errors.push(e);
            None
        }
    };
    let reference = metadata(reference_root, "olo-chrome");
    let candidate = metadata(candidate_root, "olo-browser");
    if let (Some(r), Some(c)) = (&reference, &candidate) {
        if (&r.resource_archive_sha256, &r.fonts_sha256, &r.machine, &r.os, &r.locale, &r.timezone) != (&c.resource_archive_sha256, &c.fonts_sha256, &c.machine, &c.os, &c.locale, &c.timezone) {
            metadata_errors.push("reference and candidate environments/resources differ".into());
        }
        if r.binary_sha256 == c.binary_sha256 {
            metadata_errors.push("reference and candidate are the same binary".into());
        }
    }
    let mut report = Report { schema_version:1, scope:"Curated 100-site coverage; not an estimate of whole-web compatibility. Token/standard conformance is separate. No masks; pixel delta >8, at most 0.01% changed pixels, mean delta <=0.05; geometry within 0.5 CSS px. Live scale changes require separate coverage.".into(), suite_sha256, reference, candidate, metadata_errors, total:0, passed:0, failed:0, blocked:0, missing:0, sites_total:suite.sites.len(), sites_passed:0, cases:Vec::new() };
    for site in &suite.sites {
        let mut site_passed = true;
        for viewport in &suite.matrix.viewports {
            for theme in &suite.matrix.themes {
                for scale in &suite.matrix.scales {
                    let relative = std::path::PathBuf::from(&site.id).join(&viewport.id).join(theme).join(scale.to_string());
                    let roots = [reference_root.join(&relative), candidate_root.join(&relative)];
                    let observations = roots.iter().map(|p| read_json::<Observation>(&p.join("observation.json"))).collect::<Vec<_>>();
                    let mut case = CaseResult {
                        site: site.id.clone(),
                        viewport: viewport.id.clone(),
                        scale: *scale,
                        theme: theme.clone(),
                        outcome: Outcome::Failed,
                        reasons: Vec::new(),
                        pixels: BTreeMap::new(),
                    };
                    match (&observations[0], &observations[1]) {
                        (Ok(r), Ok(c)) => {
                            for (who, o) in [("reference", r), ("candidate", c)] {
                                if o.schema_version != 1 || o.site != site.id || o.viewport != viewport.id || o.scale != *scale || o.theme != *theme {
                                    case.reasons.push(format!("{who}: observation identity mismatch"));
                                }
                                if o.status != "completed" {
                                    case.reasons.push(format!("{who}: {}: {}", o.status, o.blocked_reason.as_deref().unwrap_or("unspecified")));
                                }
                            }
                            if r.status == "blocked" || c.status == "blocked" {
                                case.outcome = Outcome::Blocked;
                            } else {
                                compare_observations(site, r, c, &mut case.reasons);
                                for id in CHECKPOINTS {
                                    match compare_png(
                                        &roots[0].join(format!("{id}.png")),
                                        &roots[1].join(format!("{id}.png")),
                                        ((viewport.width * scale + 50) / 100, (viewport.height * scale + 50) / 100),
                                    ) {
                                        Ok(diff) => {
                                            if !diff.passes() {
                                                case.reasons.push(format!("{id}: pixel difference exceeds tolerance"));
                                            }
                                            case.pixels.insert(id.into(), diff);
                                        }
                                        Err(e) => {
                                            case.reasons.push(format!("{id}: {e}"));
                                        }
                                    }
                                }
                                for (who, root) in ["reference", "candidate"].iter().zip(&roots) {
                                    match compare_png(&root.join("interaction.png"), &root.join("scrolled.png"), ((viewport.width * scale + 50) / 100, (viewport.height * scale + 50) / 100)) {
                                        Ok(diff) if diff.changed_pixels >= 64 => {}
                                        Ok(_) => case.reasons.push(format!("{who}: scrolling did not change captured pixels")),
                                        Err(e) => case.reasons.push(format!("{who}: cannot verify scroll captures: {e}")),
                                    }
                                }
                            }
                        }
                        _ => {
                            case.outcome = Outcome::Missing;
                            for (who, o) in ["reference", "candidate"].iter().zip(observations) {
                                if let Err(e) = o {
                                    case.reasons.push(format!("{who}: missing/invalid evidence: {e}"));
                                }
                            }
                        }
                    }
                    if !report.metadata_errors.is_empty() {
                        case.reasons.push("run metadata missing/invalid; see metadata_errors".into());
                    }
                    if case.reasons.is_empty() {
                        case.outcome = Outcome::Passed;
                    }
                    match case.outcome {
                        Outcome::Passed => report.passed += 1,
                        Outcome::Failed => report.failed += 1,
                        Outcome::Blocked => report.blocked += 1,
                        Outcome::Missing => report.missing += 1,
                    }
                    if !matches!(case.outcome, Outcome::Passed) {
                        site_passed = false;
                    }
                    report.total += 1;
                    report.cases.push(case);
                }
            }
        }
        report.sites_passed += usize::from(site_passed);
    }
    Ok(report)
}
