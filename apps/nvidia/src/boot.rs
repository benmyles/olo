//! RPM Fusion requests omission of the NVIDIA driver from the boot
//! image. The installed image may predate that request. Look through
//! its cpio members too, including the archive after early microcode.
use crate::modules::NAMES;
use anyhow::{Context, Result, bail, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    fs,
    io::{self, BufRead, BufReader, Read},
    path::{Component, Path, PathBuf},
};

const EFI: &str = "sys/firmware/efi";
pub fn secure_boot(root: &Path) -> Result<bool> {
    let efi = root.join(EFI);
    if !efi.try_exists()? {
        ensure!(root.join("sys/firmware").is_dir(), "cannot read the system firmware to check Secure Boot");
        return Ok(false);
    }
    let bytes = fs::read(efi.join("efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c")).context("cannot read the firmware's Secure Boot setting")?;
    match bytes.as_slice() {
        [_, _, _, _, 0] => Ok(false),
        [_, _, _, _, 1] => Ok(true),
        _ => bail!("unrecognized Secure Boot setting"),
    }
}

// These two observations are necessary for an unsigned candidate on the
// supported running Fedora system. The boot deployment must separately
// establish the policy of the kernel and command line it will select.
pub fn running_unsigned_policy(root: &Path) -> Result<()> {
    let sig = fs::read_to_string(root.join("sys/module/module/parameters/sig_enforce")).context("cannot determine kernel module signature enforcement")?;
    ensure!(sig.trim() == "N", "kernel module signature enforcement is enabled or unknown");
    let lockdown = fs::read_to_string(root.join("sys/kernel/security/lockdown")).context("cannot determine kernel lockdown policy")?;
    let mut active = lockdown.split_ascii_whitespace().filter(|word| word.starts_with('[') || word.ends_with(']'));
    ensure!(active.next() == Some("[none]") && active.next().is_none(), "kernel lockdown is enabled or unknown");
    Ok(())
}

fn omission(root: &Path) -> Result<()> {
    let mut conf = BTreeMap::new();
    for dir in ["usr/lib/dracut/dracut.conf.d", "etc/dracut.conf.d"] {
        let entries = match fs::read_dir(root.join(dir)) {
            Ok(v) => v,
            Err(e) if e.kind() == io::ErrorKind::NotFound => continue,
            Err(e) => return Err(e.into()),
        };
        for e in entries {
            let path = e?.path();
            if path.extension().is_some_and(|s| s == "conf") {
                conf.insert(path.file_name().unwrap().to_owned(), path);
            }
        }
    }
    let main = root.join("etc/dracut.conf");
    let mut omit = BTreeSet::new();
    for path in main.try_exists()?.then_some(main).into_iter().chain(conf.into_values()) {
        for line in fs::read_to_string(&path)?.lines() {
            let line = line.split('#').next().unwrap().trim();
            if !line.contains("omit_drivers") {
                continue;
            }
            let (append, value) = if let Some(v) = line.strip_prefix("omit_drivers+=") {
                (true, v)
            } else if let Some(v) = line.strip_prefix("omit_drivers=") {
                (false, v)
            } else {
                bail!("cannot interpret omit_drivers in {}", path.display())
            };
            let v = value.trim();
            let literal = v.strip_prefix('"').and_then(|v| v.strip_suffix('"')).or_else(|| v.strip_prefix('\'').and_then(|v| v.strip_suffix('\''))).unwrap_or(v);
            ensure!(literal.bytes().all(|b| b.is_ascii_alphanumeric() || b.is_ascii_whitespace() || b"_-/.".contains(&b)), "cannot interpret omit_drivers in {}", path.display());
            if !append {
                omit.clear();
            }
            omit.extend(literal.split_ascii_whitespace().map(|s| s.replace('_', "-")));
        }
    }
    // RPM Fusion omits these four. peermem depends on the core; also
    // search all five names in the actual image below.
    ensure!(NAMES[..4].iter().all(|n| omit.contains(*n)), "check dracut's omit_drivers for nvidia, nvidia-drm, nvidia-modeset and nvidia-uvm");
    Ok(())
}

fn skip(r: &mut impl Read, n: u64) -> Result<()> {
    ensure!(io::copy(&mut r.take(n), &mut io::sink())? == n, "incomplete cpio file");
    Ok(())
}

fn cpio(r: &mut impl Read) -> Result<()> {
    loop {
        let mut h = [0; 110];
        r.read_exact(&mut h)?;
        ensure!(matches!(&h[..6], b"070701" | b"070702"), "unrecognized cpio entry");
        let field = |at| -> Result<u64> { Ok(u64::from_str_radix(std::str::from_utf8(&h[at..at + 8])?, 16)?) };
        let len = field(94)?;
        ensure!((1..=libc::PATH_MAX as u64).contains(&len), "invalid cpio name length");
        let size = field(54)?;
        let mut name = vec![0; len as usize];
        r.read_exact(&mut name)?;
        let end = name.iter().position(|b| *b == 0).context("unterminated boot image file name")?;
        ensure!(end != 0 && name[end..].iter().all(|b| *b == 0), "invalid cpio name padding");
        let name = &name[..end];
        skip(r, (4 - (110 + len) % 4) % 4)?;
        if name == b"TRAILER!!!" {
            ensure!(size == 0, "invalid cpio trailer");
            return Ok(());
        }
        let base = name.rsplit(|b| *b == b'/').next().unwrap();
        if let Ok(base) = std::str::from_utf8(base) {
            if let Some((stem, ext)) = base.split_once(".ko") {
                ensure!(!(ext.is_empty() || ext.starts_with('.')) || !NAMES.contains(&stem.replace('_', "-").as_str()), "the boot image contains {base}");
            }
        }
        skip(r, size + (4 - size % 4) % 4)?;
    }
}

fn padding(r: &mut impl BufRead) -> Result<bool> {
    loop {
        let buf = r.fill_buf()?;
        if buf.is_empty() {
            return Ok(false);
        }
        let n = buf.iter().take_while(|b| **b == 0).count();
        if n == 0 {
            return Ok(true);
        }
        r.consume(n);
    }
}

fn compressed(r: impl Read) -> Result<()> {
    let mut r = BufReader::new(r);
    ensure!(padding(&mut r)?, "compressed boot image is empty");
    loop {
        cpio(&mut r)?;
        if !padding(&mut r)? {
            return Ok(());
        }
    }
}

fn images(root: &Path, release: &str) -> Result<BTreeSet<PathBuf>> {
    let mut images = BTreeSet::new();
    // Unified images carry their own initramfs. A loose .img beside
    // one says nothing about what the firmware will start.
    for folder in ["boot/efi/EFI/Linux", "efi/EFI/Linux", "boot/EFI/Linux"] {
        let path = root.join(folder);
        if path.try_exists()? {
            for e in fs::read_dir(path)? {
                let p = e?.path();
                ensure!(!p.extension().is_some_and(|e| e.eq_ignore_ascii_case("efi")), "check the initramfs embedded in {}: unified kernel images are not supported", p.display());
            }
        }
    }
    let boot = root.join("boot");
    let mut saw_entry = false;
    for partition in [&boot, &root.join("efi"), &boot.join("efi")] {
        let entries = partition.join("loader/entries");
        if !entries.try_exists()? {
            continue;
        }
        for e in fs::read_dir(entries)? {
            let path = e?.path();
            if path.extension().is_none_or(|e| e != "conf") {
                continue;
            }
            saw_entry = true;
            let text = fs::read_to_string(&path)?;
            let values = |key: &str| -> Vec<&str> { text.lines().map(str::trim).filter_map(|l| l.split_once(char::is_whitespace)).filter(|(k, _)| *k == key).map(|(_, v)| v.trim()).collect() };
            let linux = values("linux");
            // Only skip an entry positively identified with another release.
            // Unidentified entries can share the target kernel or its image.
            if let [file] = linux.as_slice() {
                let p = Path::new(file);
                let base = p.file_name().and_then(|v| v.to_str());
                let identified = match base {
                    Some("linux") => p.parent().and_then(Path::file_name).and_then(|v| v.to_str()),
                    Some(name) => name.strip_prefix("vmlinuz-"),
                    None => None,
                };
                if identified.is_some_and(|v| crate::system::valid_release(v).is_ok() && v != release) && !values("version").contains(&release) {
                    continue;
                }
            }
            let listed = values("initrd");
            ensure!(linux.len() == 1 && ["efi", "uki", "uki-url"].iter().all(|key| values(key).is_empty()) && !listed.is_empty(), "check the boot image named by {}", path.display());
            for name in listed {
                let rel = Path::new(name.trim_start_matches('/'));
                ensure!(rel.components().all(|c| matches!(c, Component::Normal(_))) && !name.contains(['$', '%', '*']), "cannot resolve initrd {name} in {}", path.display());
                images.insert(partition.join(rel));
            }
        }
    }
    if images.is_empty() {
        ensure!(!saw_entry, "no boot entry identifies the images for {release}");
        images.insert(boot.join(format!("initramfs-{release}.img")));
    }
    Ok(images)
}

pub fn initramfs(root: &Path, release: &str) -> Result<()> {
    omission(root)?;
    for image in images(root, release)? {
        inspect(&image).with_context(|| format!("check the boot image {}", image.display()))?;
    }
    Ok(())
}
fn inspect(image: &Path) -> Result<()> {
    let mut r = BufReader::new(fs::File::open(image)?);
    let mut seen = false;
    while padding(&mut r)? {
        let mut magic = [0; 6];
        r.read_exact(&mut magic)?;
        let mut member = magic.as_slice().chain(&mut r);
        if magic.starts_with(b"07070") {
            cpio(&mut member)?;
        } else if magic.starts_with(b"\x28\xb5\x2f\xfd") {
            compressed(zstd::stream::read::Decoder::with_buffer(&mut member)?.single_frame())?;
        } else if magic.starts_with(b"\x1f\x8b") {
            compressed(flate2::bufread::GzDecoder::new(&mut member))?;
        } else if magic.starts_with(b"\xfd7zXZ\0") {
            compressed(liblzma::bufread::XzDecoder::new(&mut member))?;
        } else {
            bail!("unrecognized boot image format; check {} for NVIDIA modules", image.display());
        }
        seen = true;
    }
    ensure!(seen, "{} is empty", image.display());
    Ok(())
}
