use anyhow::Context as _;
use blade_graphics as gpu;
use std::sync::{Arc, Mutex};
use util::ResultExt;

#[cfg_attr(target_os = "macos", derive(Clone))]
pub struct BladeContext {
    pub(super) gpu: Arc<gpu::Context>,
    /// Olo: the pipelines windows draw with, shared (see [`PipelineCache`]).
    pub(super) pipelines: Arc<Mutex<PipelineCache>>,
    /// Olo: the glyphs and pictures windows draw, one atlas for them all:
    /// a window opened (a menu, the command palette) finds what the
    /// others put there, rather than rasterizing and uploading it all
    /// again for its first frame. Its keys hold all a tile depends on
    /// (size, scale, the display's text rendering).
    pub(super) atlas: Arc<super::BladeAtlas>,
}

/// Olo: the pipelines windows draw with, made once for each kind of
/// surface (its format and alpha, and the paths' sample count) and kept
/// while the GPU is: making them (the shader compiled, ten pipelines)
/// took about 30 ms, which every window opened paid (a menu, the command
/// palette at each ⌘Space).
#[derive(Default)]
pub(super) struct PipelineCache {
    /// GPUI's shader, compiled (WGSL through naga: ~3 ms), once.
    shader: Option<Arc<gpu::Shader>>,
    pub(super) sets: Vec<((gpu::SurfaceInfo, u32), Arc<super::blade_renderer::BladePipelines>)>,
    /// LCD subpixel glyphs' (the second-source blend), by format.
    pub(super) subpixel: Vec<(gpu::TextureFormat, Arc<gpu::RenderPipeline>)>,
}

impl PipelineCache {
    /// GPUI's shader, compiled the first time.
    pub(super) fn shader(&mut self, gpu: &gpu::Context) -> Arc<gpu::Shader> {
        self.shader.get_or_insert_with(|| Arc::new(gpu.create_shader(gpu::ShaderDesc { source: include_str!("shaders.wgsl") }))).clone()
    }
}

impl BladeContext {
    /// Olo: made off the thread that will open the first window, with
    /// its shader compiled, while the app starts (the context ~4.4 ms and
    /// the shader ~3 ms went after the first window was asked for).
    pub fn ahead() -> std::thread::JoinHandle<anyhow::Result<BladeContext>> {
        std::thread::spawn(|| {
            let context = BladeContext::new()?;
            context.pipelines.lock().unwrap_or_else(|e| e.into_inner()).shader(&context.gpu);
            Ok(context)
        })
    }

    pub fn new() -> anyhow::Result<Self> {
        let device_id_forced = match std::env::var("ZED_DEVICE_ID") {
            Ok(val) => parse_pci_id(&val).context("Failed to parse device ID from `ZED_DEVICE_ID` environment variable").log_err(),
            Err(std::env::VarError::NotPresent) => None,
            err => {
                err.context("Failed to read value of `ZED_DEVICE_ID` environment variable").log_err();
                None
            }
        };
        // Olo: a discrete GPU asleep beside the one the displays use is
        // left asleep: its Vulkan driver isn't loaded (`asleep_aside`).
        let aside = asleep_aside();
        let made = unsafe {
            gpu::Context::init(gpu::ContextDesc {
                presentation: true,
                validation: false,
                // Olo: each pass timed on the GPU, printed a frame at
                // a time by the renderer (for measuring).
                timing: super::gpu_timings(),
                device_id: device_id_forced.unwrap_or(0),
                ..Default::default()
            })
        };
        drop(aside);
        let gpu = Arc::new(made.map_err(|e| anyhow::anyhow!("{e:?}"))?);
        let atlas = Arc::new(super::BladeAtlas::new(&gpu));
        let holds: Arc<dyn crate::PlatformAtlas> = atlas.clone();
        crate::assets::holds_images(&holds);
        Ok(Self { gpu, pipelines: Arc::default(), atlas })
    }
}

/// Olo: NVIDIA's Vulkan driver, loaded to list the devices, wakes its GPU
/// from its sleep (D3cold) and waits for it: every app's start took 2.3 s
/// longer on a laptop whose NVIDIA GPU slept (a ThinkPad P1 Gen 9,
/// 2026-11-04), and kept it awake (power) for a GPU it never draws with:
/// windows are drawn by the GPU the displays are composed on, the
/// integrated one. So while the context is made, the Vulkan loader
/// leaves NVIDIA's driver out (`VK_LOADER_DRIVERS_DISABLE`), when there
/// is another GPU to draw with and the person hasn't chosen drivers or
/// asked for NVIDIA (PRIME offload); the variable is put back as it was
/// as soon as the loader has found its drivers, so nothing the app
/// starts inherits it.
fn asleep_aside() -> Option<impl Drop> {
    const DISABLE: &str = "VK_LOADER_DRIVERS_DISABLE";
    let chosen = [DISABLE, "VK_LOADER_DRIVERS_SELECT", "VK_DRIVER_FILES", "VK_ICD_FILENAMES", "__NV_PRIME_RENDER_OFFLOAD"];
    if chosen.iter().any(|v| std::env::var_os(v).is_some()) {
        return None;
    }
    // (sysfs only: nothing here wakes a GPU.)
    let drivers: Vec<String> = std::fs::read_dir("/sys/class/drm")
        .ok()?
        .flatten()
        .filter(|e| {
            let n = e.file_name();
            let n = n.to_string_lossy();
            n.starts_with("card") && !n.contains('-')
        })
        .filter_map(|e| std::fs::read_link(e.path().join("device/driver")).ok())
        .filter_map(|d| d.file_name().map(|n| n.to_string_lossy().into_owned()))
        .collect();
    if !drivers.iter().any(|d| d == "nvidia") || !drivers.iter().any(|d| d != "nvidia") {
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
    // the app starts. Another thread may read the environment meanwhile:
    // glibc keeps an environment array it replaces, so such a reader sees
    // the variable or not (one only Vulkan's loader reads).
    unsafe { std::env::set_var(DISABLE, "*nvidia*") };
    Some(Restore)
}

fn parse_pci_id(id: &str) -> anyhow::Result<u32> {
    let mut id = id.trim();

    if id.starts_with("0x") || id.starts_with("0X") {
        id = &id[2..];
    }
    let is_hex_string = id.chars().all(|c| c.is_ascii_hexdigit());
    let is_4_chars = id.len() == 4;
    anyhow::ensure!(is_4_chars && is_hex_string, "Expected a 4 digit PCI ID in hexadecimal format");

    u32::from_str_radix(id, 16).context("parsing PCI ID as hex")
}

#[cfg(test)]
mod tests {
    use super::parse_pci_id;

    #[test]
    fn test_parse_device_id() {
        assert!(parse_pci_id("0xABCD").is_ok());
        assert!(parse_pci_id("ABCD").is_ok());
        assert!(parse_pci_id("abcd").is_ok());
        assert!(parse_pci_id("1234").is_ok());
        assert!(parse_pci_id("123").is_err());
        assert_eq!(parse_pci_id(&format!("{:x}", 0x1234)).unwrap(), parse_pci_id(&format!("{:X}", 0x1234)).unwrap(),);

        assert_eq!(parse_pci_id(&format!("{:#x}", 0x1234)).unwrap(), parse_pci_id(&format!("{:#X}", 0x1234)).unwrap(),);
        assert_eq!(parse_pci_id(&format!("{:#x}", 0x1234)).unwrap(), parse_pci_id(&format!("{:#X}", 0x1234)).unwrap(),);
    }
}
