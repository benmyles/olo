//! The three reviewed changes, plus RPM Fusion's one change to the
//! open interface. A hunk applies at its stated line with every byte
//! of its context; looking around for somewhere similar is unsafe here.
use anyhow::{Context, Result, bail, ensure};
use sha2::{Digest, Sha256};
use std::{collections::BTreeMap, fs, path::Path};

macro_rules! patch {
    ($name:literal) => {
        ($name, include_str!(concat!("../../../kernel/nvidia/patches/", $name)))
    };
}
pub const PREVENTION: [(&str, &str); 3] = [patch!("0001-gsp-local-halt-deadline.patch"), patch!("0002-pci-reserve-remove-stack-at-probe.patch"), patch!("0003-pm-abort-before-engine-unload.patch")];
const SUMS: &str = include_str!("../../../kernel/nvidia/series.sha256");
const FEDORA: &str = include_str!("../fedora-open.patch");
// The kernel-open hunk from set_driver_defaults.patch in 615.71.09-3,
// whose full digest is in kernel/nvidia/prepare-and-build.sh.
const FEDORA_SHA: &str = "09959e2eb7b71694e84ab165db3780d92772e243ad547df42b0a66ce336cb1de";

pub fn hashes() -> Result<BTreeMap<String, String>> {
    let mut hashes = BTreeMap::new();
    for (name, text) in PREVENTION {
        let expected = SUMS.lines().filter_map(|line| line.split_once("  patches/")).find(|(_, n)| *n == name).context("patch missing from series.sha256")?.0;
        ensure!(hex::encode(Sha256::digest(text)) == expected, "{name} differs from series.sha256");
        hashes.insert(name.into(), expected.into());
    }
    ensure!(hex::encode(Sha256::digest(FEDORA)) == FEDORA_SHA, "the Fedora preparation patch changed");
    hashes.insert("fedora-open.patch".into(), FEDORA_SHA.into());
    Ok(hashes)
}

fn range(token: &str, prefix: char) -> Result<(usize, usize)> {
    let (start, count) = token.strip_prefix(prefix).context("invalid hunk header")?.split_once(',').context("missing hunk size")?;
    Ok((start.parse::<usize>()?.checked_sub(1).context("invalid hunk start")?, count.parse()?))
}

pub fn apply(dir: &Path, patch: &str) -> Result<()> {
    // The input is embedded and its digest has been checked. Only the
    // simple file edits in this release are supported.
    let lines: Vec<_> = patch.split_inclusive('\n').collect();
    let mut i = 0;
    let mut files = 0;
    while i < lines.len() {
        let Some(old) = lines[i].strip_prefix("--- a/") else {
            i += 1;
            continue;
        };
        let name = old.split_ascii_whitespace().next().context("missing file name")?;
        ensure!(Path::new(name).components().all(|p| matches!(p, std::path::Component::Normal(_))), "invalid patch path");
        i += 1;
        ensure!(lines.get(i).and_then(|l| l.strip_prefix("+++ b/")).and_then(|l| l.split_ascii_whitespace().next()) == Some(name), "patch changes the file name");
        let path = dir.join(name);
        let original = fs::read_to_string(&path).with_context(|| format!("reading {name} for patching"))?;
        let before: Vec<_> = original.split_inclusive('\n').collect();
        let mut after = Vec::new();
        let mut cursor = 0;
        i += 1;
        ensure!(lines.get(i).is_some_and(|l| l.starts_with("@@ ")), "missing patch hunk");
        while let Some(header) = lines.get(i).and_then(|l| l.strip_prefix("@@ ")) {
            let mut words = header.split_ascii_whitespace();
            let (start, removed) = range(words.next().context("missing old range")?, '-')?;
            let (dest, added) = range(words.next().context("missing new range")?, '+')?;
            after.extend_from_slice(before.get(cursor..start).context("patch range outside file")?);
            ensure!(after.len() == dest, "patch's new line number disagrees in {name}");
            cursor = start;
            let mut count = 0;
            i += 1;
            while cursor - start < removed || count < added {
                let (kind, content) = lines.get(i).and_then(|l| l.split_at_checked(1)).context("short patch")?;
                if kind == " " || kind == "-" {
                    ensure!(before.get(cursor) == Some(&content), "patch context differs in {name} at line {}", cursor + 1);
                    cursor += 1;
                }
                match kind {
                    " " | "+" => {
                        after.push(content);
                        count += 1;
                    }
                    "-" => {}
                    _ => bail!("invalid hunk in {name}"),
                }
                i += 1;
            }
            ensure!(cursor - start == removed && count == added, "patch hunk size disagrees in {name}");
        }
        after.extend_from_slice(&before[cursor..]);
        fs::write(path, after.concat())?;
        files += 1;
    }
    ensure!(files > 0, "patch has no file edits");
    Ok(())
}

pub fn prepare(dir: &Path) -> Result<()> {
    hashes()?;
    for (name, patch) in PREVENTION.into_iter().chain([("fedora-open.patch", FEDORA)]) {
        apply(dir, patch).with_context(|| format!("applying {name}"))?;
    }
    Ok(())
}
