use std::{
    collections::BTreeMap,
    fs::File,
    path::{Path, PathBuf},
};

use olo_web_eval::{CHECKPOINTS, Checkpoint, Cost, Observation, Outcome, Rect, RunMetadata, Suite, compare_png, evaluate};
use sha2::{Digest, Sha256};

const SUITE: &str = include_str!("../../../docs/browser/eval/sites.json");

fn json(path: &Path, value: &impl serde::Serialize) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(path, serde_json::to_vec(value).unwrap()).unwrap();
}

fn png(path: &Path, shift: usize, blank: bool, alpha: u8) {
    let mut pixels = vec![255; 1280 * 800 * 4];
    for y in 100..200 {
        for x in (100 + shift)..(200 + shift) {
            let p = (y * 1280 + x) * 4;
            if !blank {
                pixels[p..p + 3].fill(0);
            }
            pixels[p + 3] = alpha;
        }
    }
    let mut encoder = png::Encoder::new(File::create(path).unwrap(), 1280, 800);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.write_header().unwrap().write_image_data(&pixels).unwrap();
}

struct Fixture {
    _dir: tempfile::TempDir,
    suite: PathBuf,
    reference: PathBuf,
    candidate: PathBuf,
    reference_case: PathBuf,
    candidate_case: PathBuf,
    observation: Observation,
    metadata: RunMetadata,
}

impl Fixture {
    fn new(populate: bool) -> Self {
        let dir = tempfile::Builder::new().prefix("olo-web-eval-").tempdir_in("/var/tmp").unwrap();
        let suite = dir.path().join("suite.json");
        std::fs::write(&suite, SUITE).unwrap();
        let reference = dir.path().join("reference");
        let candidate = dir.path().join("candidate");
        let case = "google-com/desktop/light/100";
        let reference_case = reference.join(case);
        let candidate_case = candidate.join(case);
        let parsed: Suite = serde_json::from_str(SUITE).unwrap();
        let initial = "Meaningful synthetic test content for an initial browser document, not an actual website result.";
        let next = "Different synthetic test content for an activated browser document, not an actual website result.";
        let checkpoints = CHECKPOINTS
            .iter()
            .map(|&id| {
                let first = matches!(id, "loaded" | "history");
                (
                    id.into(),
                    Checkpoint {
                        text: if first { initial } else { next }.into(),
                        document_url: if first { "https://google.com/" } else { "https://google.com/result" }.into(),
                        document_generation: if first { 1 } else { 2 },
                        scroll: [0.0, if id == "scrolled" { 200.0 } else { 0.0 }],
                        landmarks: (0..3).map(|i| (format!("landmark-{i}"), Rect { x: 100.0, y: 100.0 + i as f64 * 20.0, width: 100.0, height: 20.0 })).collect(),
                    },
                )
            })
            .collect();
        let observation = Observation {
            schema_version: 1,
            site: "google-com".into(),
            viewport: "desktop".into(),
            scale: 100,
            theme: "light".into(),
            status: "completed".into(),
            blocked_reason: None,
            assertions: parsed.sites[0].checks.iter().map(|c| (c.id.clone(), true)).collect(),
            checkpoints,
            cost: Cost { navigation_ms: 100.0, first_input_ms: 1.0, worst_frame_ms: 2.0, peak_rss_kib: 10_000, idle_duration_ms: 1000, idle_frames: 0, idle_cpu_ms: 0.0, steady_rss_growth_kib: 0 },
        };
        let metadata = RunMetadata {
            schema_version: 1,
            engine: "olo-chrome".into(),
            version: "synthetic-fixture".into(),
            binary_sha256: "1".repeat(64),
            source_revision: "synthetic-fixture".into(),
            suite_sha256: format!("{:x}", Sha256::digest(SUITE)),
            resource_archive_sha256: "2".repeat(64),
            fonts_sha256: "3".repeat(64),
            machine: "test".into(),
            os: "test".into(),
            locale: "en-US".into(),
            timezone: "UTC".into(),
            captured_at: "2026-11-04T00:00:00Z".into(),
            release: true,
        };
        let f = Self { _dir: dir, suite, reference, candidate, reference_case, candidate_case, observation, metadata };
        if populate {
            json(&f.reference.join("run.json"), &f.metadata);
            let mut candidate_metadata = f.metadata.clone();
            candidate_metadata.engine = "olo-browser".into();
            candidate_metadata.binary_sha256 = "4".repeat(64);
            json(&f.candidate.join("run.json"), &candidate_metadata);
            for root in [&f.reference_case, &f.candidate_case] {
                json(&root.join("observation.json"), &f.observation);
                for id in CHECKPOINTS {
                    png(&root.join(format!("{id}.png")), if id == "scrolled" { 200 } else { 0 }, false, 255);
                }
            }
        }
        f
    }

    fn report(&self) -> olo_web_eval::Report {
        evaluate(&self.suite, &self.reference, &self.candidate).unwrap()
    }
}

#[test]
fn missing_evidence_never_passes_or_changes_the_denominator() {
    let f = Fixture::new(false);
    let report = f.report();
    assert_eq!((report.total, report.passed, report.missing, report.sites_total, report.sites_passed), (3000, 0, 3000, 100, 0));
    assert_eq!(report.metadata_errors.len(), 2);
}

#[test]
fn rejects_omitted_scales_duplicate_sites_and_missing_workflow_assertions() {
    let suite: Suite = serde_json::from_str(SUITE).unwrap();
    suite.validate().unwrap();
    let mut changed = suite.clone();
    changed.matrix.scales.remove(2);
    assert!(changed.validate().is_err());
    let mut changed = suite.clone();
    changed.sites[1] = changed.sites[0].clone();
    assert!(changed.validate().is_err());
    let mut changed = suite;
    changed.sites[0].checks[0].id = "other".into();
    assert!(changed.validate().is_err());
}

#[test]
fn paired_case_requires_complete_behavior_geometry_pixels_metadata_and_cost() {
    let f = Fixture::new(true);
    let report = f.report();
    assert_eq!((report.passed, report.missing, report.sites_passed), (1, 2999, 0));
    assert!(matches!(report.cases[0].outcome, Outcome::Passed), "{:?}", report.cases[0].reasons);

    let mut changed = f.observation.clone();
    changed.assertions.remove("search_results");
    json(&f.candidate_case.join("observation.json"), &changed);
    assert_eq!(f.report().passed, 0, "missing assertion");

    let mut changed = f.observation.clone();
    changed.checkpoints.get_mut("loaded").unwrap().landmarks.get_mut("landmark-0").unwrap().x += 1.0;
    json(&f.candidate_case.join("observation.json"), &changed);
    assert_eq!(f.report().passed, 0, "geometry shift");

    let mut changed = f.observation.clone();
    changed.cost.idle_frames = 1;
    json(&f.candidate_case.join("observation.json"), &changed);
    assert_eq!(f.report().passed, 0, "idle work");

    let mut changed = f.observation.clone();
    changed.checkpoints.get_mut("scrolled").unwrap().scroll = [0.0, 0.0];
    json(&f.candidate_case.join("observation.json"), &changed);
    assert_eq!(f.report().passed, 0, "assertions cannot substitute for scroll evidence");

    let mut changed = f.observation.clone();
    changed.status = "blocked".into();
    changed.blocked_reason = Some("dedicated account unavailable".into());
    json(&f.candidate_case.join("observation.json"), &changed);
    let report = f.report();
    assert_eq!((report.passed, report.blocked, report.missing), (0, 1, 2999));

    json(&f.candidate_case.join("observation.json"), &f.observation);
    png(&f.candidate_case.join("loaded.png"), 100, false, 255);
    assert_eq!(f.report().passed, 0, "pixel shift");
    png(&f.candidate_case.join("loaded.png"), 0, false, 255);
    png(&f.candidate_case.join("scrolled.png"), 0, false, 255);
    assert_eq!(f.report().passed, 0, "unchanged scroll screenshot");
    png(&f.candidate_case.join("scrolled.png"), 200, false, 255);

    let mut metadata = f.metadata.clone();
    metadata.engine = "olo-browser".into();
    metadata.binary_sha256 = "4".repeat(64);
    metadata.fonts_sha256 = "5".repeat(64);
    json(&f.candidate.join("run.json"), &metadata);
    assert_eq!(f.report().passed, 0, "environment mismatch");
}

#[test]
fn png_comparison_rejects_matching_blanks_wrong_dimensions_transparency_and_truncation() {
    let dir = tempfile::Builder::new().prefix("olo-web-pixels-").tempdir_in("/var/tmp").unwrap();
    let a = dir.path().join("a.png");
    let b = dir.path().join("b.png");
    png(&a, 0, true, 255);
    png(&b, 0, true, 255);
    assert!(!compare_png(&a, &b, (1280, 800)).unwrap().passes());
    assert!(compare_png(&a, &b, (640, 400)).is_err());
    png(&a, 0, false, 0);
    assert!(compare_png(&a, &b, (1280, 800)).is_err());
    png(&a, 0, false, 255);
    png(&b, 0, false, 255);
    assert!(compare_png(&a, &b, (1280, 800)).unwrap().passes());
    let mut bytes = std::fs::read(&b).unwrap();
    bytes.truncate(bytes.len() / 2);
    std::fs::write(&b, bytes).unwrap();
    assert!(compare_png(&a, &b, (1280, 800)).is_err());
}

#[test]
fn invalid_observation_cannot_silently_be_a_valid_blank() {
    let f = Fixture::new(true);
    let mut value = serde_json::to_value(&f.observation).unwrap();
    value["cost"]["typo"] = serde_json::json!(1);
    json(&f.candidate_case.join("observation.json"), &value);
    assert_eq!(f.report().passed, 0);
    let mut changed = f.observation.clone();
    changed.checkpoints = BTreeMap::new();
    json(&f.candidate_case.join("observation.json"), &changed);
    assert_eq!(f.report().passed, 0);
}
