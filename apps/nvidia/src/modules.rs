//! A set on disk, the index's choice for the next start, and the build
//! ID of the core already in the kernel. All are ordinary file reads.
use anyhow::{Context, Result, bail, ensure};
use object::{Object, ObjectSection};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};

pub const NAMES: [&str; 5] = ["nvidia", "nvidia-drm", "nvidia-modeset", "nvidia-uvm", "nvidia-peermem"];
pub const STOCK: &str = "extra/nvidia";
pub const OURS: &str = "updates/olo";
pub type Set = BTreeMap<String, Module>;
pub type Paths = BTreeMap<String, PathBuf>;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Module {
    pub version: String,
    pub vermagic: String,
    pub build_id: String,
    pub sha256: String,
}

pub fn module_name(path: &Path) -> Option<&str> {
    let name = path.file_name()?.to_str()?;
    let (stem, rest) = name.split_once(".ko")?;
    (matches!(rest, "" | ".xz" | ".zst") && NAMES.contains(&stem)).then_some(stem)
}

pub fn paths(dir: &Path) -> Result<Paths> {
    let mut paths = Paths::new();
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let path = entry?.path();
        if let Some(name) = module_name(&path) {
            ensure!(paths.insert(name.to_owned(), path).is_none(), "two copies of a module in {}", dir.display());
        }
    }
    for name in NAMES {
        ensure!(paths.contains_key(name), "{name} is missing from {}", dir.display());
    }
    Ok(paths)
}

pub fn read(path: &Path, name: &str) -> Result<Module> {
    let file = File::open(path)?;
    let reader: Box<dyn Read> = match path.extension().and_then(|e| e.to_str()) {
        Some("xz") => Box::new(liblzma::read::XzDecoder::new(file)),
        Some("zst") => Box::new(zstd::Decoder::new(file)?),
        Some("ko") => Box::new(file),
        _ => bail!("unknown module compression"),
    };
    let mut bytes = Vec::new();
    const LIMIT: u64 = 256 << 20;
    reader.take(LIMIT + 1).read_to_end(&mut bytes)?;
    ensure!(bytes.len() as u64 <= LIMIT, "module exceeds 256 MiB");
    let elf = object::File::parse(bytes.as_slice())?;
    ensure!(elf.format() == object::BinaryFormat::Elf && elf.kind() == object::ObjectKind::Relocatable, "expected a relocatable ELF module");
    let info = elf.section_by_name(".modinfo").context("no ELF .modinfo section")?.data()?;
    let field = |key: &str| -> Result<String> {
        let mut fields = info.split(|b| *b == 0).filter_map(|s| s.strip_prefix(key.as_bytes()));
        let first = fields.next().with_context(|| format!("no {key} in .modinfo"))?;
        ensure!(!first.is_empty() && fields.next().is_none(), "invalid {key} in .modinfo");
        Ok(std::str::from_utf8(first)?.into())
    };
    ensure!(field("name=")?.replace('_', "-") == name, "the file holds another module");
    let build_id = elf.build_id()?.filter(|id| !id.is_empty()).context("no GNU build ID")?;
    Ok(Module { version: field("version=")?, vermagic: field("vermagic=")?, build_id: hex::encode(build_id), sha256: hex::encode(Sha256::digest(&bytes)) })
}

pub fn set(dir: &Path, release: &str) -> Result<Set> {
    let mut set = Set::new();
    for (name, path) in paths(dir)? {
        let module = read(&path, &name).with_context(|| format!("reading {}", path.display()))?;
        ensure!(module.vermagic.split_ascii_whitespace().next() == Some(release), "{name} was built for a different kernel ({})", module.vermagic);
        if let Some(first) = set.values().next() {
            ensure!(first.version == module.version && first.vermagic == module.vermagic, "the five modules in {} disagree on their version or vermagic", dir.display());
        }
        set.insert(name, module);
    }
    Ok(set)
}

/// depmod writes one left-hand entry for each name it selected. A file
/// mentioned as another module's dependency alone is no evidence.
pub fn index(kernel: &Path) -> Result<Paths> {
    let mut found = Paths::new();
    for line in fs::read_to_string(kernel.join("modules.dep")).context("reading modules.dep")?.lines() {
        let Some((p, _)) = line.split_once(':') else { bail!("invalid line in modules.dep") };
        let path = Path::new(p);
        if let Some(name) = module_name(path) {
            ensure!(found.insert(name.into(), path.into()).is_none(), "modules.dep lists {name} twice");
        }
    }
    Ok(found)
}

pub fn verify_index(kernel: &Path, dir: &str) -> Result<()> {
    let expected = paths(&kernel.join(dir))?;
    let selected = index(kernel)?;
    verify_binary(kernel, &selected)?;
    for (name, target) in expected {
        ensure!(selected.get(&name).is_some_and(|p| kernel.join(p) == target), "modules.dep did not select {} for {name}", target.display());
    }
    Ok(())
}

pub fn loaded_id(note: &[u8]) -> Option<String> {
    let len = |offset: usize| Some(u32::from_ne_bytes(note.get(offset..offset + 4)?.try_into().ok()?));
    if len(0)? != 4 || len(8)? != 3 || note.get(12..16)? != b"GNU\0" {
        return None;
    }
    let size = len(4)? as usize;
    (size != 0).then(|| note.get(16..16 + size).map(hex::encode)).flatten()
}

/// libkmod's read-only name lookup checks the binary index used at
/// boot. A correct text index alone cannot detect an interrupted copy
/// of modules.dep.bin. Passing an empty config list omits host aliases.
pub fn verify_binary(kernel: &Path, text: &Paths) -> Result<()> {
    use std::ffi::{CStr, CString, OsStr, c_char, c_void};
    use std::os::unix::ffi::OsStrExt;
    #[link(name = "libkmod.so.2", kind = "dylib", modifiers = "+verbatim")]
    unsafe extern "C" {
        fn kmod_new(dir: *const c_char, config: *const *const c_char) -> *mut c_void;
        fn kmod_unref(ctx: *mut c_void) -> *mut c_void;
        fn kmod_module_new_from_name_lookup(ctx: *mut c_void, name: *const c_char, module: *mut *mut c_void) -> i32;
        fn kmod_module_get_path(module: *mut c_void) -> *const c_char;
        fn kmod_module_unref(module: *mut c_void) -> *mut c_void;
    }
    struct ContextHandle(*mut c_void);
    impl Drop for ContextHandle {
        fn drop(&mut self) {
            unsafe { kmod_unref(self.0) };
        }
    }
    let dir = CString::new(kernel.as_os_str().as_bytes())?;
    let config = [std::ptr::null()];
    let ctx = ContextHandle(unsafe { kmod_new(dir.as_ptr(), config.as_ptr()) });
    ensure!(!ctx.0.is_null(), "cannot open the kernel's module index");
    for name in NAMES {
        let n = CString::new(name)?;
        let mut m = std::ptr::null_mut();
        let ret = unsafe { kmod_module_new_from_name_lookup(ctx.0, n.as_ptr(), &mut m) };
        ensure!(ret >= 0, "cannot look up {name} in the kernel's binary index ({ret})");
        let path = if m.is_null() {
            None
        } else {
            let path = unsafe { kmod_module_get_path(m) };
            let value = if path.is_null() { None } else { Some(PathBuf::from(OsStr::from_bytes(unsafe { CStr::from_ptr(path) }.to_bytes()))) };
            unsafe { kmod_module_unref(m) };
            value
        };
        ensure!(path == text.get(name).map(|p| kernel.join(p)), "the text and binary indexes disagree for {name}");
    }
    Ok(())
}
