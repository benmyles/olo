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

/// Variables that say the person chose the drivers, or NVIDIA: left
/// alone then.
const CHOSEN: [&str; 6] = ["VK_LOADER_DRIVERS_DISABLE", "VK_LOADER_DRIVERS_SELECT", "VK_DRIVER_FILES", "VK_ICD_FILENAMES", "__EGL_VENDOR_LIBRARY_FILENAMES", "__NV_PRIME_RENDER_OFFLOAD"];

/// Mesa's EGL vendor (glvnd's), the one left when NVIDIA's is set aside.
const MESA_EGL: [&str; 2] = ["/usr/share/glvnd/egl_vendor.d/50_mesa.json", "/etc/glvnd/egl_vendor.d/50_mesa.json"];

/// The DRM cards' drivers ("xe", "nvidia").
fn drivers(drm: &Path) -> Vec<String> {
    std::fs::read_dir(drm)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("card") && !n.contains('-')
        })
        .filter_map(|e| std::fs::read_link(e.path().join("device/driver")).ok())
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect()
}

/// Whether an NVIDIA GPU (NVIDIA's driver) sits beside another GPU.
pub fn nvidia_beside_another() -> bool {
    beside(&drivers(Path::new("/sys/class/drm")))
}

fn beside(drivers: &[String]) -> bool {
    drivers.iter().any(|d| d == "nvidia") && drivers.iter().any(|d| d != "nvidia")
}

/// The variables to start a program with so it leaves NVIDIA's GPU asleep
/// (none if there's no such GPU, or the person chose otherwise).
pub fn asleep_env() -> Vec<(&'static str, String)> {
    if CHOSEN.iter().any(|v| std::env::var_os(v).is_some()) || !nvidia_beside_another() {
        return Vec::new();
    }
    let mut env = vec![("VK_LOADER_DRIVERS_DISABLE", "*nvidia*".to_owned())];
    if let Some(mesa) = MESA_EGL.iter().find(|p| Path::new(p).exists()) {
        env.push(("__EGL_VENDOR_LIBRARY_FILENAMES", (*mesa).to_owned()));
    }
    env
}

#[cfg(test)]
mod tests {
    #[test]
    fn beside_another() {
        let d = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(super::beside(&d(&["xe", "nvidia"])));
        assert!(!super::beside(&d(&["nvidia"])));
        assert!(!super::beside(&d(&["xe"])));
        assert!(!super::beside(&d(&["amdgpu", "i915"])));
    }
}
