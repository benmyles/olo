//! One Vulkan driver, handed to the loader directly (Olo's).
//!
//! The Vulkan loader finds its drivers by loading every one installed
//! and asking each what it has: on a desktop with Mesa a dozen
//! libraries, and NVIDIA's where its driver is installed, which wakes
//! its card to answer (2.3 s a start on a laptop whose card slept, and
//! the card kept awake for a GPU the program never draws with). Olo
//! first kept NVIDIA's out with the loader's own variable
//! (`nvidia_aside`, 2026-11-04), which told a card of NVIDIA's by its
//! DRM card: on 2026-11-20, with that card's display module stuck
//! unloading and its DRM card gone, every program loaded NVIDIA's
//! driver again, for a card that no longer answered (a window 684 ms
//! after its program started where it takes 55, and a program crashed
//! in NVIDIA's library as it exited).
//!
//! So a program is given the one driver of the GPU it draws on: the one
//! the computer started on, which is the one the first suitable device
//! was always taken from (where that driver is one of Mesa's:
//! `libraries_of`). Nothing of that depends on what else is installed
//! or what state it is in. The library is found from the loader's own
//! manifests, loaded here, and handed to the loader with
//! `VK_LUNARG_direct_driver_loading`, exclusively: the loader then
//! loads no other. What the driver has is asked of the driver (the
//! loader can't say it before the instance exists, and asking the
//! loader is what loads every driver).
//!
//! Measured (release builds, a test desktop on the GPU, ten starts each
//! in turn against the loader's way with NVIDIA's left out): a window
//! is shown 23 and 30 ms after its program starts where it took 55 and
//! 58, and a program has 3 MB less of its own written to (the loader's
//! way leaves lavapipe and LLVM mapped beside the driver in use).
//!
//! It steps aside, and the loader does as it always did (NVIDIA's left
//! out still, `nvidia_aside`), wherever that isn't plainly right: a
//! loader variable that chooses drivers is set (a test's lavapipe, a
//! person's own choice: the exclusive list would override it), the
//! person asked for NVIDIA (`__NV_PRIME_RENDER_OFFLOAD`),
//! `OLO_VULKAN_DRIVERS=all`, no one GPU the computer started on, a
//! kernel driver not known here, no manifest for it, a library that
//! won't load or hasn't the entry; and `Context::init` goes the
//! ordinary way again if the instance made so can't be made or has no
//! device it can use.

use ash::vk;
use std::{
    ffi, fs,
    path::{Path, PathBuf},
};

/// The loader's variables that choose drivers, which an exclusive list
/// would make of no effect; and NVIDIA's own, by which a person asks
/// for a program to be drawn on its card (PRIME render offload).
const CHOSEN: &[&str] = &[
    "VK_DRIVER_FILES",
    "VK_ICD_FILENAMES",
    "VK_ADD_DRIVER_FILES",
    "VK_LOADER_DRIVERS_SELECT",
    "VK_LOADER_DRIVERS_DISABLE",
    "__NV_PRIME_RENDER_OFFLOAD",
];

type GetInstanceProcAddr =
    unsafe extern "system" fn(vk::Instance, *const ffi::c_char) -> vk::PFN_vkVoidFunction;
type EnumerateInstanceExtensions = unsafe extern "system" fn(
    *const ffi::c_char,
    *mut u32,
    *mut vk::ExtensionProperties,
) -> vk::Result;

/// The drivers to hand the loader, loaded: for the program's life (an
/// instance made of them may outlive anything here, one that failed
/// part way too, and unloading a driver under an instance is what must
/// never happen; a driver is one library, mapped once).
pub(super) struct Drivers {
    infos: Vec<vk::DirectDriverLoadingInfoLUNARG<'static>>,
    /// The instance extensions they have, as they say.
    extensions: Vec<ffi::CString>,
    /// Their files' names, for the log.
    pub names: Vec<String>,
}

impl Drivers {
    /// What goes on the instance's chain.
    pub fn list(&self) -> vk::DirectDriverLoadingListLUNARG<'_> {
        vk::DirectDriverLoadingListLUNARG::default()
            .mode(vk::DirectDriverLoadingModeLUNARG::EXCLUSIVE)
            .drivers(&self.infos)
    }

    pub fn extensions(&self) -> Vec<&ffi::CStr> {
        self.extensions.iter().map(|e| e.as_c_str()).collect()
    }
}

/// While the loader finds its drivers the ordinary way (where one
/// couldn't be handed over, `find`): NVIDIA's left out, on a computer
/// that has another GPU to draw with. Loaded to list its devices,
/// NVIDIA's driver wakes its card from its sleep and waits for it:
/// every program's start took 2.3 s longer on a laptop whose card slept
/// (a ThinkPad P1 Gen 9, 2026-11-04), and kept it awake for a GPU the
/// program never draws with. The loader's variable for it
/// (`VK_LOADER_DRIVERS_DISABLE`) is set until what is returned is
/// dropped, so nothing the program starts inherits it; not where the
/// person chose drivers or asked for NVIDIA themselves (`CHOSEN`).
///
/// That there is a card of NVIDIA's is read from the PCI bus, not from
/// its DRM card: its display module gone (unloaded, or stuck unloading
/// with its card dead, 2026-11-20), the card and its Vulkan driver are
/// still there, and every program loaded it.
pub(super) fn nvidia_aside() -> Option<impl Drop> {
    const DISABLE: &str = "VK_LOADER_DRIVERS_DISABLE";
    if CHOSEN.iter().any(|name| std::env::var_os(name).is_some())
        || !nvidia_beside_another(Path::new("/sys"))
    {
        return None;
    }
    struct Restore;
    impl Drop for Restore {
        fn drop(&mut self) {
            // SAFETY: as below.
            unsafe { std::env::remove_var(DISABLE) };
        }
    }
    // SAFETY: set for the moment the loader reads it, then removed, as
    // the program starts. Another thread may read the environment
    // meanwhile: glibc keeps an environment array it replaces, so such a
    // reader sees the variable or not (one only Vulkan's loader reads).
    unsafe { std::env::set_var(DISABLE, "*nvidia*") };
    Some(Restore)
}

/// Whether there is a card of NVIDIA's under `sys` (a `/sys`; a PCI
/// display device of its) and another GPU to draw with (a DRM card
/// whose driver isn't NVIDIA's).
pub(super) fn nvidia_beside_another(sys: &Path) -> bool {
    let read = |p: PathBuf| fs::read_to_string(p).unwrap_or_default().trim().to_owned();
    let nvidia = fs::read_dir(sys.join("bus/pci/devices"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            read(e.path().join("vendor")) == "0x10de"
                && read(e.path().join("class")).starts_with("0x03")
        });
    let another = fs::read_dir(sys.join("class/drm"))
        .into_iter()
        .flatten()
        .flatten()
        .any(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.strip_prefix("card")
                .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
                && fs::read_link(e.path().join("device/driver"))
                    .is_ok_and(|d| d.file_name().is_some_and(|n| n != "nvidia"))
        });
    nvidia && another
}

/// The drivers of the GPU the computer started on, if this is the way
/// to go here (the module's words).
pub(super) fn find() -> Option<Drivers> {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if CHOSEN.iter().any(|name| set(name))
        || std::env::var("OLO_VULKAN_DRIVERS").as_deref() == Ok("all")
    {
        return None;
    }
    let driver = boot_driver(Path::new("/sys"))?;
    let wanted = libraries_of(&driver);
    if wanted.is_empty() {
        log::info!("Vulkan: no driver known here for the kernel's {driver}: every driver asked");
        return None;
    }
    let found = installed(wanted, &manifest_dirs());
    let mut drivers = Drivers {
        infos: Vec::new(),
        extensions: Vec::new(),
        names: Vec::new(),
    };
    for path in found {
        // SAFETY: a Vulkan driver the loader would load itself.
        let Ok(library) = (unsafe { libloading::Library::new(&path) }) else {
            continue;
        };
        // SAFETY: the driver interface's entry, of this type.
        let entry =
            match unsafe { library.get::<GetInstanceProcAddr>(b"vk_icdGetInstanceProcAddr\0") } {
                Ok(entry) => *entry,
                Err(_) => continue,
            };
        // (Loaded for good: the struct's words.)
        std::mem::forget(library);
        // SAFETY: the entry asked for a global function by name.
        for name in unsafe { extensions_of(entry) } {
            if !drivers.extensions.contains(&name) {
                drivers.extensions.push(name);
            }
        }
        drivers.infos.push(
            vk::DirectDriverLoadingInfoLUNARG::default().pfn_get_instance_proc_addr(Some(entry)),
        );
        drivers.names.push(path.file_name().map_or_else(
            || path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        ));
    }
    if drivers.infos.is_empty() {
        log::info!(
            "Vulkan: the kernel's {driver} has no driver installed that loads: every driver asked"
        );
        return None;
    }
    Some(drivers)
}

/// The instance extensions a driver has, asked of the driver.
unsafe fn extensions_of(entry: GetInstanceProcAddr) -> Vec<ffi::CString> {
    let Some(function) = entry(
        vk::Instance::null(),
        b"vkEnumerateInstanceExtensionProperties\0".as_ptr().cast(),
    ) else {
        return Vec::new();
    };
    let enumerate: EnumerateInstanceExtensions = std::mem::transmute(function);
    let mut count = 0;
    if enumerate(std::ptr::null(), &mut count, std::ptr::null_mut()) != vk::Result::SUCCESS {
        return Vec::new();
    }
    let mut properties = vec![vk::ExtensionProperties::default(); count as usize];
    let result = enumerate(std::ptr::null(), &mut count, properties.as_mut_ptr());
    if result != vk::Result::SUCCESS && result != vk::Result::INCOMPLETE {
        return Vec::new();
    }
    properties.truncate(count as usize);
    properties
        .iter()
        .map(|p| ffi::CStr::from_ptr(p.extension_name.as_ptr()).to_owned())
        .collect()
}

/// The kernel driver of the GPU the computer started on, under `sys` (a
/// `/sys`): the card whose device says `boot_vga`, or the only card
/// there is (where no device says it: not a PCI one). None where there
/// is no one such.
pub(super) fn boot_driver(sys: &Path) -> Option<String> {
    let mut cards: Vec<(bool, String)> = Vec::new();
    for entry in fs::read_dir(sys.join("class/drm")).ok()?.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        // ("card0", not "card0-eDP-1", a connector.)
        if !name
            .strip_prefix("card")
            .is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
        {
            continue;
        }
        let device = entry.path().join("device");
        let Some(driver) = fs::read_link(device.join("driver"))
            .ok()
            .and_then(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        else {
            continue;
        };
        let boot = fs::read_to_string(device.join("boot_vga")).is_ok_and(|b| b.trim() == "1");
        cards.push((boot, driver));
    }
    let mut boot = cards.iter().filter(|(boot, _)| *boot);
    match (boot.next(), boot.next()) {
        (Some((_, driver)), None) => Some(driver.clone()),
        (None, _) if cards.len() == 1 => Some(cards[0].1.clone()),
        _ => None,
    }
}

/// The Vulkan drivers' libraries for a kernel driver, by their files'
/// names: Mesa's. Two for i915: which of them takes the GPU is the
/// GPU's generation, which each driver knows of itself. Not NVIDIA's
/// own driver (the kernel's `nvidia`): whether its library can be
/// handed over this way hasn't been tried on a computer that draws
/// with it, so there the loader does as it always did.
pub(super) fn libraries_of(kernel_driver: &str) -> &'static [&'static str] {
    match kernel_driver {
        "xe" => &["libvulkan_intel.so"],
        "i915" => &["libvulkan_intel.so", "libvulkan_intel_hasvk.so"],
        "amdgpu" | "radeon" => &["libvulkan_radeon.so"],
        "nouveau" => &["libvulkan_nouveau.so"],
        "asahi" => &["libvulkan_asahi.so"],
        "virtio-pci" | "virtio_gpu" => &["libvulkan_virtio.so"],
        "msm" | "msm_dpu" => &["libvulkan_freedreno.so"],
        "panfrost" | "panthor" => &["libvulkan_panfrost.so"],
        "v3d" | "vc4" => &["libvulkan_broadcom.so"],
        "powervr" => &["libvulkan_powervr_mesa.so"],
        _ => &[],
    }
}

/// Where the loader looks for drivers' manifests on Linux, in its
/// order: the configuration folders, then the data folders.
fn manifest_dirs() -> Vec<PathBuf> {
    let var = |name: &str| std::env::var_os(name).filter(|v| !v.is_empty());
    let home = var("HOME").map(PathBuf::from);
    let mut dirs: Vec<PathBuf> = Vec::new();
    dirs.extend(
        var("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".config"))),
    );
    match var("XDG_CONFIG_DIRS") {
        Some(list) => dirs.extend(std::env::split_paths(&list)),
        None => dirs.push("/etc/xdg".into()),
    }
    dirs.push("/etc".into());
    dirs.extend(
        var("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| home.as_ref().map(|h| h.join(".local/share"))),
    );
    match var("XDG_DATA_DIRS") {
        Some(list) => dirs.extend(std::env::split_paths(&list)),
        None => dirs.extend(["/usr/local/share".into(), "/usr/share".into()]),
    }
    dirs.into_iter().map(|d| d.join("vulkan/icd.d")).collect()
}

/// Of the drivers the manifests in `dirs` name, those whose library's
/// file is one of `wanted`, in `wanted`'s order, each once: its path as
/// the loader would load it (a manifest's own, one beside the manifest,
/// or a bare name for the system to find).
pub(super) fn installed(wanted: &[&str], dirs: &[PathBuf]) -> Vec<PathBuf> {
    let mut named: Vec<PathBuf> = Vec::new();
    for dir in dirs {
        let Ok(entries) = fs::read_dir(dir) else {
            continue;
        };
        let mut files: Vec<PathBuf> = entries
            .flatten()
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x == "json"))
            .collect();
        files.sort();
        for file in files {
            let Some(library) = fs::read_to_string(&file)
                .ok()
                .and_then(|text| library_path(&text))
            else {
                continue;
            };
            let library = PathBuf::from(library);
            named.push(
                if library.is_absolute() || library.components().count() == 1 {
                    library
                } else {
                    dir.join(library)
                },
            );
        }
    }
    let mut found = Vec::new();
    for name in wanted {
        if let Some(path) = named
            .iter()
            .find(|p| p.file_name().is_some_and(|n| n == *name))
        {
            if !found.contains(path) {
                found.push(path.clone());
            }
        }
    }
    found
}

/// A manifest's `library_path` (its text is JSON; this reads the one
/// string, with its escapes).
pub(super) fn library_path(manifest: &str) -> Option<String> {
    let rest = &manifest[manifest.find("\"library_path\"")? + "\"library_path\"".len()..];
    let rest = rest
        .trim_start()
        .strip_prefix(':')?
        .trim_start()
        .strip_prefix('"')?;
    let mut out = String::new();
    let mut chars = rest.chars();
    loop {
        match chars.next()? {
            '"' => return Some(out),
            '\\' => match chars.next()? {
                'n' => out.push('\n'),
                't' => out.push('\t'),
                'u' => {
                    let code: String = chars.by_ref().take(4).collect();
                    out.push(char::from_u32(u32::from_str_radix(&code, 16).ok()?)?);
                }
                other => out.push(other),
            },
            c => out.push(c),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("blade-direct-{name}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn card(sys: &Path, name: &str, driver: &str, boot: Option<&str>) {
        let device = sys.join("devices").join(name);
        fs::create_dir_all(&device).unwrap();
        fs::create_dir_all(sys.join("drivers").join(driver)).unwrap();
        std::os::unix::fs::symlink(sys.join("drivers").join(driver), device.join("driver"))
            .unwrap();
        if let Some(boot) = boot {
            fs::write(device.join("boot_vga"), format!("{boot}\n")).unwrap();
        }
        fs::create_dir_all(sys.join("class/drm").join(name)).unwrap();
        std::os::unix::fs::symlink(&device, sys.join("class/drm").join(name).join("device"))
            .unwrap();
    }

    /// The GPU the computer started on: the card that says so; where
    /// none does, the only card; never a guess between two.
    #[test]
    fn the_gpu_the_computer_started_on() {
        let sys = scratch("boot");
        assert_eq!(boot_driver(&sys), None);
        card(&sys, "card0", "xe", Some("1"));
        card(&sys, "card1", "nvidia", Some("0"));
        // (A connector is not a card.)
        fs::create_dir_all(sys.join("class/drm/card0-eDP-1")).unwrap();
        assert_eq!(boot_driver(&sys).as_deref(), Some("xe"));
        // No device says it (not PCI ones) and there are two: not known.
        let sys = scratch("boot-two");
        card(&sys, "card0", "asahi", None);
        card(&sys, "card1", "virtio_gpu", None);
        assert_eq!(boot_driver(&sys), None);
        // One card that doesn't say: that one.
        let sys = scratch("boot-one");
        card(&sys, "card0", "asahi", None);
        assert_eq!(boot_driver(&sys).as_deref(), Some("asahi"));
        // Two that both say it (it happens with two cards of one kind): not known.
        let sys = scratch("boot-both");
        card(&sys, "card0", "amdgpu", Some("1"));
        card(&sys, "card1", "amdgpu", Some("1"));
        assert_eq!(boot_driver(&sys), None);
    }

    /// NVIDIA's card is known by the PCI bus: with its DRM card gone
    /// (its display module unloaded, or stuck unloading) it is still
    /// beside the GPU that draws.
    #[test]
    fn nvidias_card_beside_another_gpu() {
        let sys = scratch("aside");
        let pci = |address: &str, vendor: &str, class: &str| {
            let dir = sys.join("bus/pci/devices").join(address);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("vendor"), format!("{vendor}\n")).unwrap();
            fs::write(dir.join("class"), format!("{class}\n")).unwrap();
        };
        assert!(!nvidia_beside_another(&sys));
        card(&sys, "card0", "xe", Some("1"));
        pci("0000:00:02.0", "0x8086", "0x030000");
        assert!(!nvidia_beside_another(&sys), "no card of NVIDIA's");
        // (Its sound function isn't a card.)
        pci("0000:01:00.1", "0x10de", "0x040300");
        assert!(!nvidia_beside_another(&sys));
        pci("0000:01:00.0", "0x10de", "0x030000");
        assert!(
            nvidia_beside_another(&sys),
            "known with no DRM card of its own"
        );
        card(&sys, "card1", "nvidia", Some("0"));
        assert!(nvidia_beside_another(&sys));
        // Only NVIDIA's to draw with: never left out.
        let sys = scratch("aside-only");
        let dir = sys.join("bus/pci/devices/0000:01:00.0");
        fs::create_dir_all(&dir).unwrap();
        fs::write(dir.join("vendor"), "0x10de\n").unwrap();
        fs::write(dir.join("class"), "0x030000\n").unwrap();
        card(&sys, "card0", "nvidia", Some("1"));
        assert!(!nvidia_beside_another(&sys));
    }

    #[test]
    fn a_manifests_library() {
        let intel = "{\n    \"ICD\": {\n        \"api_version\": \"1.4.335\",\n        \"library_arch\": \"64\",\n        \"library_path\": \"/usr/lib64/libvulkan_intel.so\"\n    },\n    \"file_format_version\": \"1.0.1\"\n}";
        assert_eq!(
            library_path(intel).as_deref(),
            Some("/usr/lib64/libvulkan_intel.so")
        );
        assert_eq!(
            library_path("{\"ICD\":{\"library_path\":\"libGLX_nvidia.so.0\"}}").as_deref(),
            Some("libGLX_nvidia.so.0")
        );
        assert_eq!(
            library_path(r#"{"ICD": {"library_path" : "C:\\drivers\\a\/b\u0041.dll"}}"#).as_deref(),
            Some("C:\\drivers\\a/bA.dll")
        );
        assert_eq!(library_path("{\"ICD\": {}}"), None);
        assert_eq!(library_path("{\"library_path\": \"unfinished"), None);
    }

    /// The drivers installed for a kernel driver, from manifests as the
    /// loader reads them: an absolute path, one beside its manifest, a
    /// bare name; the first folder's wins; what isn't wanted isn't taken.
    #[test]
    fn the_drivers_installed_for_a_kernel_driver() {
        let root = scratch("manifests");
        let (etc, share) = (
            root.join("etc/vulkan/icd.d"),
            root.join("share/vulkan/icd.d"),
        );
        fs::create_dir_all(&etc).unwrap();
        fs::create_dir_all(&share).unwrap();
        let manifest = |dir: &Path, name: &str, library: &str| {
            fs::write(
                dir.join(name),
                format!(
                    "{{\"ICD\": {{\"library_path\": \"{library}\", \"api_version\": \"1.4.0\"}}}}"
                ),
            )
            .unwrap()
        };
        manifest(
            &share,
            "intel_icd.x86_64.json",
            "/usr/lib64/libvulkan_intel.so",
        );
        manifest(
            &share,
            "intel_hasvk_icd.x86_64.json",
            "/usr/lib64/libvulkan_intel_hasvk.so",
        );
        manifest(&share, "lvp_icd.x86_64.json", "/usr/lib64/libvulkan_lvp.so");
        manifest(&share, "nvidia_icd.x86_64.json", "libGLX_nvidia.so.0");
        manifest(
            &share,
            "radeon_icd.x86_64.json",
            "../../../lib64/libvulkan_radeon.so",
        );
        fs::write(
            share.join("notes.txt"),
            "\"library_path\": \"/usr/lib64/libvulkan_intel.so\"",
        )
        .unwrap();
        let dirs = [etc.clone(), share.clone(), root.join("nowhere")];
        assert_eq!(
            installed(libraries_of("xe"), &dirs),
            [PathBuf::from("/usr/lib64/libvulkan_intel.so")]
        );
        assert_eq!(
            installed(libraries_of("i915"), &dirs),
            [
                PathBuf::from("/usr/lib64/libvulkan_intel.so"),
                PathBuf::from("/usr/lib64/libvulkan_intel_hasvk.so")
            ]
        );
        // (A bare name, for the system to find: as a manifest may say.)
        assert_eq!(
            installed(&["libGLX_nvidia.so.0"], &dirs),
            [PathBuf::from("libGLX_nvidia.so.0")]
        );
        // NVIDIA's own driver isn't handed over: the loader's way there.
        assert!(libraries_of("nvidia").is_empty());
        assert_eq!(
            installed(libraries_of("amdgpu"), &dirs),
            [share.join("../../../lib64/libvulkan_radeon.so")]
        );
        assert!(installed(libraries_of("nouveau"), &dirs).is_empty());
        assert!(installed(libraries_of("simpledrm"), &dirs).is_empty());
        // An administrator's manifest for the same driver comes first.
        manifest(
            &etc,
            "intel_icd.x86_64.json",
            "/opt/mesa/lib64/libvulkan_intel.so",
        );
        assert_eq!(
            installed(libraries_of("xe"), &dirs),
            [PathBuf::from("/opt/mesa/lib64/libvulkan_intel.so")]
        );
    }
}
