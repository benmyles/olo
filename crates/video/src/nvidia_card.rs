//! Whether NVIDIA's graphics card can be used at all, asked of the
//! kernel's own files before any library of NVIDIA's is loaded or
//! started. A card whose power management failed answers nothing until
//! the computer restarts, and one whose driver is being unloaded is
//! going away (or stuck going): starting CUDA for either gets an error
//! at best (2026-11-20: HACKING, "A graphics card that stopped
//! answering"), and the menus that offer the card's encoder and decoder
//! shouldn't offer what can't work.

use std::path::Path;

/// Err with why where no NVIDIA card here can be used now: none with
/// its driver, one that stopped answering, or its driver on its way
/// out. One asleep is fine: using it is what wakes it.
pub(crate) fn usable() -> Result<(), String> {
    usable_in(Path::new("/sys"))
}

fn usable_in(sys: &Path) -> Result<(), String> {
    let read = |p: std::path::PathBuf| std::fs::read_to_string(p).unwrap_or_default().trim().to_owned();
    for module in ["nvidia_drm", "nvidia_modeset", "nvidia_uvm", "nvidia_peermem", "nvidia"] {
        if read(sys.join("module").join(module).join("initstate")) == "going" {
            return Err("NVIDIA's driver is being unloaded".into());
        }
    }
    let mut cards = 0;
    for dir in std::fs::read_dir(sys.join("bus/pci/devices")).into_iter().flatten().flatten().map(|e| e.path()) {
        if read(dir.join("vendor")) != "0x10de" || !read(dir.join("class")).starts_with("0x03") {
            continue;
        }
        if !std::fs::read_link(dir.join("driver")).is_ok_and(|l| l.file_name().is_some_and(|n| n == "nvidia")) {
            continue;
        }
        if read(dir.join("power/runtime_status")) == "error" {
            return Err("NVIDIA's graphics card stopped answering (its power management failed): restarting the computer starts it again".into());
        }
        cards += 1;
    }
    if cards == 0 {
        return Err("no NVIDIA graphics card with its driver here".into());
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_card_is_usable_unless_it_failed_or_its_driver_is_going() {
        let sys = std::env::temp_dir().join(format!("olo-video-nvidia-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&sys);
        let w = |p: &str, c: &str| {
            let path = sys.join(p);
            std::fs::create_dir_all(path.parent().unwrap()).unwrap();
            std::fs::write(path, c).unwrap();
        };
        // Nothing of NVIDIA's here.
        assert!(usable_in(&sys).is_err());
        let card = "bus/pci/devices/0000:01:00.0";
        w(&format!("{card}/vendor"), "0x10de\n");
        w(&format!("{card}/class"), "0x030000\n");
        w(&format!("{card}/power/runtime_status"), "suspended\n");
        // A card with no driver of NVIDIA's (nouveau's, or none).
        assert!(usable_in(&sys).is_err());
        std::fs::create_dir_all(sys.join("bus/pci/drivers/nvidia")).unwrap();
        std::os::unix::fs::symlink(sys.join("bus/pci/drivers/nvidia"), sys.join(card).join("driver")).unwrap();
        // Asleep, awake: usable.
        assert_eq!(usable_in(&sys), Ok(()));
        w(&format!("{card}/power/runtime_status"), "active\n");
        assert_eq!(usable_in(&sys), Ok(()));
        // Its power management failed.
        w(&format!("{card}/power/runtime_status"), "error\n");
        assert!(usable_in(&sys).is_err_and(|e| e.contains("stopped answering")));
        // Its driver on its way out, whatever the card says.
        w(&format!("{card}/power/runtime_status"), "active\n");
        w("module/nvidia_uvm/initstate", "going\n");
        assert!(usable_in(&sys).is_err_and(|e| e.contains("unloaded")));
        w("module/nvidia_uvm/initstate", "live\n");
        assert_eq!(usable_in(&sys), Ok(()));
        let _ = std::fs::remove_dir_all(&sys);
    }
}
