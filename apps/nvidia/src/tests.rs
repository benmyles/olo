use super::*;
use anyhow::Result;
use index::{Depmod, Indexer};
use modules::{NAMES, OURS, STOCK};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
};
const K: &str = "7.2.9-test";
const NEXT: &str = "7.2.10-test";
const EFI: &str = "sys/firmware/efi/efivars/SecureBoot-8be4df61-93ca-11d2-aa0d-00e098032b8c";

fn put(p: &Path, data: impl AsRef<[u8]>) {
    fs::create_dir_all(p.parent().unwrap()).unwrap();
    fs::write(p, data).unwrap();
}
fn write_at(bytes: &mut [u8], off: usize, v: impl AsRef<[u8]>) {
    let v = v.as_ref();
    bytes[off..off + v.len()].copy_from_slice(v);
}
fn note(id: u8) -> Vec<u8> {
    [&[4, 0, 0, 0, 20, 0, 0, 0, 3, 0, 0, 0][..], b"GNU\0", &[id; 20]].concat()
}

/// A small, genuine relocatable ELF file, including the symbol table
/// read by depmod. No build tools or GPU are needed for the fixtures.
fn module(name: &str, release: &str, version: &str, id: u8) -> Vec<u8> {
    let info = format!("license=GPL\0name={}\0version={version}\0vermagic={release} SMP \0", name.replace('-', "_")).into_bytes();
    let sections = [(".shstrtab", 3, vec![]), (".modinfo", 1, info), (".note.gnu.build-id", 7, note(id)), (".strtab", 3, vec![0]), (".symtab", 2, vec![0; 24])];
    let mut strings = vec![0];
    let mut names = vec![];
    for (name, _, _) in &sections {
        names.push(strings.len() as u32);
        strings.extend(name.as_bytes());
        strings.push(0);
    }
    let mut bytes = vec![0; 64];
    write_at(&mut bytes, 0, b"\x7fELF\x02\x01\x01");
    write_at(&mut bytes, 16, 1u16.to_le_bytes());
    write_at(&mut bytes, 18, 62u16.to_le_bytes());
    write_at(&mut bytes, 20, 1u32.to_le_bytes());
    for (at, v) in [(52, 64u16), (58, 64), (60, 6), (62, 1)] {
        write_at(&mut bytes, at, v.to_le_bytes());
    }
    let mut headers = vec![0; 64];
    for (i, (_, kind, data)) in sections.into_iter().enumerate() {
        let data = if i == 0 { strings.as_slice() } else { &data };
        let mut h = [0; 64];
        bytes.resize(bytes.len().next_multiple_of(8), 0);
        write_at(&mut h, 0, names[i].to_le_bytes());
        write_at(&mut h, 4, (kind as u32).to_le_bytes());
        write_at(&mut h, 24, (bytes.len() as u64).to_le_bytes());
        write_at(&mut h, 32, (data.len() as u64).to_le_bytes());
        write_at(&mut h, 48, (if i == 2 { 4u64 } else { 1 }).to_le_bytes());
        if i == 4 {
            write_at(&mut h, 40, 4u32.to_le_bytes());
            write_at(&mut h, 44, 1u32.to_le_bytes());
            write_at(&mut h, 56, 24u64.to_le_bytes());
        }
        bytes.extend(data);
        headers.extend(h);
    }
    bytes.resize(bytes.len().next_multiple_of(8), 0);
    let offset = bytes.len() as u64;
    write_at(&mut bytes, 40, offset.to_le_bytes());
    bytes.extend(headers);
    bytes
}

fn cpio(name: &str) -> Vec<u8> {
    let mut out = Vec::new();
    for n in [name, "TRAILER!!!"] {
        let mut fields = [0; 13];
        fields[1] = 0o100644;
        fields[4] = 1;
        fields[11] = n.len() + 1;
        out.extend(b"070701");
        for f in fields {
            write!(&mut out, "{f:08x}").unwrap();
        }
        out.extend(n.as_bytes());
        out.push(0);
        out.resize(out.len().next_multiple_of(4), 0);
    }
    out
}
fn fixture_set(dir: &Path, r: &str, v: &str, id: u8) {
    for n in NAMES {
        put(&dir.join(format!("{n}.ko")), module(n, r, v, id));
    }
}

struct Tree {
    _scratch: tempfile::TempDir,
    s: System,
}
impl Tree {
    fn new() -> Self {
        let tmp = real_build::scratch();
        let s = System { root: tmp.path().into() };
        put(&s.root.join("proc/sys/kernel/osrelease"), K);
        put(&s.root.join(EFI), [6, 0, 0, 0, 0]);
        put(&s.root.join("sys/module/module/parameters/sig_enforce"), "N");
        put(&s.root.join("sys/kernel/security/lockdown"), "[none] integrity confidentiality");
        put(&s.root.join("usr/lib/dracut/dracut.conf.d/99-nvidia.conf"), "omit_drivers+=\" nvidia nvidia_drm nvidia_modeset nvidia_uvm \"\n");
        fs::create_dir_all(s.state()).unwrap();
        let t = Self { _scratch: tmp, s };
        t.kernel(K);
        t
    }
    fn kernel(&self, r: &str) {
        let k = self.s.kernel(r);
        fixture_set(&k.join(STOCK), r, build::VERSION, 1);
        for p in ["modules.order", "modules.builtin", "modules.builtin.modinfo"] {
            put(&k.join(p), "");
        }
        let h = self.s.root.join("usr/src/kernels").join(r);
        put(&h.join("Makefile"), "");
        put(&h.join("include/config/kernel.release"), r);
        put(&self.s.root.join(format!("boot/initramfs-{r}.img")), cpio("init"));
        // A distribution installation supplies the reference indexes.
        Depmod.generate(&self.s, r, true, &self.s.root).unwrap();
    }
    fn output(&self) -> PathBuf {
        let dir = self.s.root.join("compile");
        fixture_set(&dir.join("kernel-open"), K, build::VERSION, 2);
        dir
    }
    fn install(&self, tool: &dyn Indexer) -> Result<()> {
        let stock = modules::set(&self.s.kernel(K).join(STOCK), K)?;
        manage::install(&self.s, K, &self.output(), &stock, tool, true)
    }
}

#[test]
fn metadata_in_plain_xz_and_zstd_modules() {
    let t = Tree::new();
    let bytes = module("nvidia", K, build::VERSION, 123);
    let mut xz = liblzma::write::XzEncoder::new(vec![], 1);
    xz.write_all(&bytes).unwrap();
    for (suffix, data) in [("ko", bytes.clone()), ("ko.xz", xz.finish().unwrap()), ("ko.zst", zstd::encode_all(&bytes[..], 1).unwrap())] {
        let path = t.s.root.join(format!("nvidia.{suffix}"));
        put(&path, data);
        let m = modules::read(&path, "nvidia").unwrap();
        assert_eq!((m.version, m.vermagic, m.build_id), (build::VERSION.into(), format!("{K} SMP "), hex::encode([123; 20])));
        assert!(modules::read(&path, "nvidia-drm").is_err());
    }
}
#[test]
fn admission_refusals_record_the_reason_and_keep_stock() {
    let t = Tree::new();
    let s = &t.s;
    let k = s.kernel(K);
    let refused = |why: &str| {
        let e = manage::build(s, K, false, &Depmod).unwrap_err();
        assert!(format!("{e:#}").contains(why), "{e:#}");
        let o: system::Outcome = system::json(&s.outcome(K)).unwrap();
        assert!(!o.ok && o.message.contains(why));
        assert!(!k.join(OURS).exists() && !s.opted_in().unwrap());
    };
    fixture_set(&k.join(STOCK), K, "999.0", 1);
    refused("distribution has 999.0");
    t.kernel(K);
    fs::remove_file(k.join(STOCK).join("nvidia-uvm.ko")).unwrap();
    refused("nvidia-uvm is missing");
    t.kernel(K);
    fs::remove_file(s.root.join(format!("usr/src/kernels/{K}/Makefile"))).unwrap();
    refused("kernel-devel");
    t.kernel(K);
    put(&s.root.join(EFI), [6, 0, 0, 0, 1]);
    refused("Secure Boot is on");
    put(&s.root.join(EFI), [6, 0, 0]);
    refused("unrecognized Secure Boot");
    put(&s.root.join(EFI), [6, 0, 0, 0, 0]);
    let img = s.root.join(format!("boot/initramfs-{K}.img"));
    put(&img, b"unexpected");
    refused("unrecognized boot image");
    let mut gz = flate2::write::GzEncoder::new(vec![], flate2::Compression::fast());
    gz.write_all(&cpio("lib/modules/nvidia-peermem.ko.zst")).unwrap();
    let mut combined = cpio("early_cpio");
    combined.extend([0; 32]);
    combined.extend(gz.finish().unwrap());
    put(&img, combined);
    refused("boot image contains nvidia-peermem");
    t.kernel(K);
    put(&s.root.join("etc/dracut.conf.d/99-nvidia.conf"), "# mask the vendor file");
    refused("omit_drivers");
    fs::remove_file(s.root.join("etc/dracut.conf.d/99-nvidia.conf")).unwrap();
    refused("no verified source is cached");
    put(&s.state().join(format!("source/{}.tar.gz", build::COMMIT)), "changed archive");
    refused("differs from the pinned SHA-256");
}
#[test]
fn index_promotion_status_and_withdrawal() {
    let t = Tree::new();
    let s = &t.s;
    let k = s.kernel(K);
    t.install(&Depmod).unwrap();
    modules::verify_index(&k, OURS).unwrap();
    let m = manage::healthy(s, K).unwrap();
    assert_eq!(m.modules.len(), 5);
    assert_eq!(m.patches.len(), 4);
    assert!(s.opted_in().unwrap());
    let note_path = s.root.join("sys/module/nvidia/notes/.note.gnu.build-id");
    put(&s.root.join("sys/module/nvidia/initstate"), "live");
    put(&note_path, note(1));
    let a = status::status(s).unwrap();
    assert_eq!((a.kernels[0].next_boot, a.kernels[0].loaded), ("olo", Some("stock")));
    put(&note_path, note(2));
    assert_eq!(status::status(s).unwrap().kernels[0].loaded, Some("olo"));
    put(&note_path, note(3));
    assert_eq!(status::status(s).unwrap().kernels[0].loaded, Some("unknown"));
    let stock_before = modules::set(&k.join(STOCK), K).unwrap();
    manage::withdraw(s, K, &Depmod).unwrap();
    assert_eq!(modules::set(&k.join(STOCK), K).unwrap(), stock_before);
    modules::verify_index(&k, STOCK).unwrap();
    assert!(!s.manifest(K).exists() && !k.join(OURS).exists());
    assert!(s.opted_in().unwrap());
    run(s, &["withdraw".into(), "--all".into()]).unwrap();
    assert!(!s.opted_in().unwrap());
}
#[test]
fn an_unrelated_module_missing_from_both_proposed_indexes_blocks_withdrawal() {
    let t = Tree::new();
    let kernel = t.s.kernel(K);
    let storage = kernel.join("kernel/drivers/storage");
    put(&storage.join("storage.ko"), module("storage", K, build::VERSION, 3));
    Depmod.generate(&t.s, K, true, &t.s.root).unwrap();
    t.install(&Depmod).unwrap();
    struct Incomplete;
    impl Indexer for Incomplete {
        fn generate(&self, s: &System, r: &str, ours: bool, out: &Path) -> Result<()> {
            // Model a successful indexer that silently omitted a subtree.
            let storage = s.kernel(r).join("kernel/drivers/storage");
            let parked = s.root.join("parked");
            fs::rename(&storage, &parked)?;
            let result = Depmod.generate(s, r, ours, out);
            fs::rename(&parked, &storage)?;
            result
        }
    }
    let before = fs::read(kernel.join("modules.dep.bin")).unwrap();
    let err = manage::withdraw(&t.s, K, &Incomplete).unwrap_err();
    assert!(format!("{err:#}").contains("depmod omitted kernel/drivers/storage/storage.ko"));
    assert_eq!(fs::read(kernel.join("modules.dep.bin")).unwrap(), before);
    modules::verify_index(&kernel, OURS).unwrap();
}

struct BrokenIndex;
impl Indexer for BrokenIndex {
    fn generate(&self, s: &System, r: &str, ours: bool, out: &Path) -> Result<()> {
        Depmod.generate(s, r, ours, out)?;
        if ours {
            fs::write(out.join("lib/modules").join(r).join("modules.dep"), "extra/nvidia/nvidia.ko:\n")?;
        }
        Ok(())
    }
}
#[test]
fn failed_index_verification_restores_all_five_stock_modules() {
    let t = Tree::new();
    let k = t.s.kernel(K);
    assert!(format!("{:#}", t.install(&BrokenIndex).unwrap_err()).contains("all five distribution modules selected again"));
    assert!(!k.join(OURS).exists() && !t.s.manifest(K).exists() && !t.s.opted_in().unwrap());
    modules::verify_index(&k, STOCK).unwrap();
}

#[test]
fn status_distinguishes_a_mixed_set_a_departing_module_and_missing_sysfs() {
    let t = Tree::new();
    t.install(&Depmod).unwrap();
    let sys = t.s.root.join("sys/module");
    for (name, id) in [("nvidia", 2), ("nvidia_drm", 1)] {
        put(&sys.join(name).join("initstate"), "live");
        put(&sys.join(name).join("notes/.note.gnu.build-id"), note(id));
    }
    assert_eq!(status::status(&t.s).unwrap().kernels[0].loaded, Some("mixed"));
    put(&sys.join("nvidia/initstate"), "going");
    assert_eq!(status::status(&t.s).unwrap().kernels[0].loaded, Some("going"));
    fs::rename(&sys, t.s.root.join("unmounted")).unwrap();
    let row = status::status(&t.s).unwrap().kernels.remove(0);
    assert_eq!(row.loaded, Some("unknown"));
    assert!(!row.errors.is_empty());
}

#[test]
fn check_withdraws_a_changed_version_missing_file_or_changed_bytes() {
    for fault in 0..4 {
        let t = Tree::new();
        let s = &t.s;
        let k = s.kernel(K);
        t.install(&Depmod).unwrap();
        let path = k.join(OURS).join("nvidia-uvm.ko");
        match fault {
            0 => fixture_set(&k.join(STOCK), K, "620.0", 3),
            1 => fs::remove_file(path).unwrap(),
            3 => put(&k.join(OURS).join("unexpected.ko"), "unexpected"),
            _ => {
                let mut bytes = fs::read(&path).unwrap();
                bytes.push(0);
                fs::write(path, bytes).unwrap();
            }
        }
        manage::check(s, &Depmod, |_| panic!("immediately rebuilt a withdrawn set")).unwrap();
        assert!(!k.join(OURS).exists());
        modules::verify_index(&k, STOCK).unwrap();
        assert!(s.opted_in().unwrap());
    }
}
#[test]
fn new_kernel_builds_offline_only_after_opt_in() {
    let t = Tree::new();
    let s = &t.s;
    t.kernel(NEXT);
    let mut requested = vec![];
    manage::check(s, &Depmod, |r| {
        requested.push(r.to_owned());
        Ok(())
    })
    .unwrap();
    assert!(requested.is_empty());
    t.install(&Depmod).unwrap();
    manage::check(s, &Depmod, |r| {
        requested.push(r.to_owned());
        Ok(())
    })
    .unwrap();
    assert_eq!(requested, [NEXT]);
    let args = ["check".into()];
    assert!(format!("{:#}", run(s, &args).unwrap_err()).contains("no verified source is cached"));
    assert!(!s.kernel(NEXT).join(OURS).exists());
    modules::verify_index(&s.kernel(K), OURS).unwrap();
}

#[test]
fn units_use_the_installed_program_and_live_device_paths_are_read_only() {
    for (unit, command) in [(watch::SERVICE, "check"), (watch::WATCH_SERVICE, "watch")] {
        let mut section = "";
        let mut start = false;
        let mut boot = false;
        for l in unit.lines().filter(|l| !l.is_empty()) {
            if l.starts_with('[') {
                section = l;
                continue;
            }
            let (k, v) = l.split_once('=').unwrap();
            if (section, k) == ("[Service]", "ExecStart") {
                assert_eq!(v, format!("/usr/local/libexec/olo/olo-nvidia {command}"));
                start = true;
            }
            if (section, k) == ("[Install]", "WantedBy") {
                assert_eq!(v, "multi-user.target");
                boot = true;
            }
        }
        assert!(start && boot);
    }
    for e in fs::read_dir(Path::new(env!("CARGO_MANIFEST_DIR")).join("src")).unwrap() {
        let path = e.unwrap().path();
        if path.file_name().unwrap() == "tests.rs" || path.file_name().unwrap() == "real_build.rs" {
            continue;
        }
        let text = fs::read_to_string(path).unwrap();
        for forbidden in ["modprobe", "insmod", "rmmod", "init_module", "delete_module", "/dev/nvidia", "/sys/bus", "/sys/class"] {
            assert!(!text.contains(forbidden), "{forbidden}");
        }
        for line in text.lines().filter(|l| l.contains("\"sys/")) {
            assert!(!["write(", "save(", "atomic(", "remove_", "create_"].iter().any(|w| line.contains(w)));
        }
    }
    for bad in ["../oops", "7/../../bad", "7$(id)", "-a", "7\\escape"] {
        assert!(system::valid_release(bad).is_err());
    }
}

struct MissingDepmod {
    during_promotion: bool,
}
impl Indexer for MissingDepmod {
    fn generate(&self, s: &System, r: &str, ours: bool, out: &Path) -> Result<()> {
        if !self.during_promotion || ours {
            return Err(std::io::Error::from(std::io::ErrorKind::NotFound).into());
        }
        Depmod.generate(s, r, ours, out)
    }
}
#[test]
fn missing_indexer_before_or_after_the_rename_leaves_stock() {
    for during_promotion in [false, true] {
        let t = Tree::new();
        let k = t.s.kernel(K);
        index::Index::prepare(&t.s, K, false, &Depmod).unwrap().publish(&k).unwrap();
        assert!(t.install(&MissingDepmod { during_promotion }).is_err());
        modules::verify_index(&k, STOCK).unwrap();
        assert!(!k.join(OURS).exists() && !t.s.manifest(K).exists() && !t.s.opted_in().unwrap());
    }
}

#[test]
fn replace_a_complete_set_and_refuse_redirected_installation_folders() {
    let t = Tree::new();
    let s = &t.s;
    t.install(&Depmod).unwrap();
    t.install(&Depmod).unwrap();
    manage::healthy(s, K).unwrap();
    manage::withdraw(s, K, &Depmod).unwrap();
    let updates = s.kernel(K).join("updates");
    fs::remove_dir(&updates).unwrap();
    let outside = real_build::scratch();
    std::os::unix::fs::symlink(outside.path(), &updates).unwrap();
    assert!(format!("{:#}", manage::build(s, K, false, &Depmod).unwrap_err()).contains("refusing redirected"));
    assert_eq!(fs::read_dir(outside.path()).unwrap().count(), 0);
}
#[test]
fn boot_entry_images_checked_including_an_early_archive_across_a_buffer_boundary() {
    let t = Tree::new();
    let s = &t.s;
    let boot = s.root.join("boot");
    put(&boot.join("loader/entries/test.conf"), format!("version {K}\nlinux /vmlinuz-{K}\ninitrd /separate.img\n"));
    let mut image = cpio("early_cpio");
    image.resize(8191, 0);
    image.extend(zstd::encode_all(&cpio("usr/lib/modules/nvidia_uvm.ko")[..], 1).unwrap());
    put(&boot.join("separate.img"), image);
    assert!(format!("{:#}", boot::initramfs(&s.root, K).unwrap_err()).contains("boot image contains nvidia_uvm"));
    put(&boot.join("separate.img"), cpio("init"));
    boot::initramfs(&s.root, K).unwrap();
    put(&boot.join("EFI/Linux/unified.efi"), "");
    assert!(format!("{:#}", boot::initramfs(&s.root, K).unwrap_err()).contains("unified kernel images"));
}

#[test]
fn a_local_depmod_rule_that_prefers_stock_cancels_the_promotion() {
    let t = Tree::new();
    put(&t.s.root.join("etc/depmod.d/priority.conf"), "search extra updates built-in\n");
    let error = t.install(&Depmod).unwrap_err();
    assert!(format!("{error:#}").contains("all five distribution modules selected again"));
    modules::verify_index(&t.s.kernel(K), STOCK).unwrap();
    assert!(!t.s.kernel(K).join(OURS).exists());
}
