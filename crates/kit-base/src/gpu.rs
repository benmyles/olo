//! Keeping a discrete GPU asleep. On a laptop whose displays are driven
//! by its integrated GPU, the NVIDIA one sleeps (D3cold) until something
//! uses it, and anything that loads NVIDIA's drivers wakes it: Vulkan's
//! loader listing devices, glvnd's EGL choosing a vendor. Waking costs
//! the waker ~2.3 s (a ThinkPad P1 Gen 9: Chrome's GPU process, every
//! Olo app before its first frame) and the battery, for a GPU nothing
//! draws with. So the programs Olo starts are given an environment
//! without NVIDIA's drivers when another GPU is there to draw with,
//! unless the person chose drivers or asked for NVIDIA themselves (PRIME
//! offload). Olo's own apps do the same as they make their context
//! (olo-interface's `blade_context::asleep_aside`). Read from sysfs only.

use std::path::Path;

/// Is a separate proprietary NVIDIA ICD unnecessary on this seat? Only infer
/// this for the unambiguous non-NVIDIA PCI boot GPU with a render node.
pub fn nvidia_beside_another() -> bool {
    beside(Path::new("/sys"))
}

fn beside(sys: &Path) -> bool {
    let Some(pci) = std::fs::read_dir(sys.join("bus/pci/devices")).ok() else { return false };
    let mut nvidia = false;
    let mut preferred = false;
    for entry in pci {
        let Ok(entry) = entry else { return false };
        let path = entry.path();
        let read = |name| std::fs::read_to_string(path.join(name)).unwrap_or_default();
        if !read("class").trim().starts_with("0x03") {
            continue;
        }
        let vendor = read("vendor");
        if vendor.trim() == "0x10de" {
            if read("boot_vga").trim() == "1" {
                return false;
            }
            nvidia = true;
        } else if !vendor.trim().is_empty() && read("boot_vga").trim() == "1" {
            if preferred {
                return false;
            }
            preferred = std::fs::read_dir(path.join("drm")).into_iter().flatten().flatten().any(|entry| {
                let name = entry.file_name();
                name.to_string_lossy().strip_prefix("renderD").is_some_and(|n| !n.is_empty() && n.bytes().all(|b| b.is_ascii_digit()))
            });
        }
    }
    preferred && nvidia
}

/// The variables to start a program with so it leaves NVIDIA's GPU asleep
/// (none if there's no such GPU, or the person chose otherwise).
pub fn asleep_env() -> Vec<(&'static str, String)> {
    let set = |name: &str| std::env::var_os(name).is_some_and(|v| !v.is_empty());
    if !nvidia_beside_another()
        || std::env::var_os("__NV_PRIME_RENDER_OFFLOAD").is_some_and(|v| v == "1")
        || ["DRI_PRIME", "MESA_VK_DEVICE_SELECT", "VK_LOADER_DEVICE_SELECT", "ANVIL_DRM_DEVICE", "OLO_RENDER_NODE"].iter().any(|k| set(k))
    {
        return Vec::new();
    }
    let mut env = Vec::new();
    if !["VK_DRIVER_FILES", "VK_ICD_FILENAMES", "VK_ADD_DRIVER_FILES", "VK_LOADER_DRIVERS_SELECT", "VK_LOADER_DRIVERS_DISABLE"].iter().any(|k| set(k)) {
        env.push(("VK_LOADER_DRIVERS_DISABLE", "*nvidia*".to_owned()));
    }
    if !set("__EGL_VENDOR_LIBRARY_FILENAMES") && !set("__EGL_VENDOR_LIBRARY_DIRS") {
        let paths = ["/etc/glvnd/egl_vendor.d/50_mesa.json", "/usr/share/glvnd/egl_vendor.d/50_mesa.json"];
        if let Some(mesa) = paths.iter().find(|p| Path::new(p).is_file()) {
            env.push(("__EGL_VENDOR_LIBRARY_FILENAMES", (*mesa).to_owned()));
        }
    }
    env
}

#[cfg(test)]
mod tests {
    #[test]
    fn beside_another() {
        let tmp = std::env::temp_dir().join(format!("olo-chrome-gpus-{}", std::process::id()));
        let pci = tmp.join("bus/pci/devices");
        let other = pci.join("0000:00:02.0");
        let nvidia = pci.join("0000:01:00.0");
        std::fs::create_dir_all(other.join("drm/renderD128")).unwrap();
        std::fs::create_dir_all(&nvidia).unwrap();
        for (p, vendor, boot) in [(&nvidia, "0x10de", "0"), (&other, "0x8086", "1")] {
            std::fs::write(p.join("vendor"), vendor).unwrap();
            std::fs::write(p.join("class"), "0x030000").unwrap();
            std::fs::write(p.join("boot_vga"), boot).unwrap();
        }
        assert!(super::beside(&tmp));
        std::fs::write(nvidia.join("boot_vga"), "1").unwrap();
        assert!(!super::beside(&tmp));
        std::fs::remove_dir_all(tmp).unwrap();
    }
}
