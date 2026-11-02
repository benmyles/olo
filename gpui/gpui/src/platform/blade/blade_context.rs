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
    pub(super) sets: Vec<((gpu::SurfaceInfo, u32), Arc<super::blade_renderer::BladePipelines>)>,
    /// LCD subpixel glyphs' (the second-source blend), by format.
    pub(super) subpixel: Vec<(gpu::TextureFormat, Arc<gpu::RenderPipeline>)>,
}

impl BladeContext {
    pub fn new() -> anyhow::Result<Self> {
        let device_id_forced = match std::env::var("ZED_DEVICE_ID") {
            Ok(val) => parse_pci_id(&val)
                .context("Failed to parse device ID from `ZED_DEVICE_ID` environment variable")
                .log_err(),
            Err(std::env::VarError::NotPresent) => None,
            err => {
                err.context("Failed to read value of `ZED_DEVICE_ID` environment variable")
                    .log_err();
                None
            }
        };
        let gpu = Arc::new(
            unsafe {
                gpu::Context::init(gpu::ContextDesc {
                    presentation: true,
                    validation: false,
                    // Olo: each pass timed on the GPU, printed a frame at
                    // a time by the renderer (for measuring).
                    timing: super::gpu_timings(),
                    device_id: device_id_forced.unwrap_or(0),
                    ..Default::default()
                })
            }
            .map_err(|e| anyhow::anyhow!("{e:?}"))?,
        );
        let atlas = Arc::new(super::BladeAtlas::new(&gpu));
        Ok(Self { gpu, pipelines: Arc::default(), atlas })
    }
}

fn parse_pci_id(id: &str) -> anyhow::Result<u32> {
    let mut id = id.trim();

    if id.starts_with("0x") || id.starts_with("0X") {
        id = &id[2..];
    }
    let is_hex_string = id.chars().all(|c| c.is_ascii_hexdigit());
    let is_4_chars = id.len() == 4;
    anyhow::ensure!(
        is_4_chars && is_hex_string,
        "Expected a 4 digit PCI ID in hexadecimal format"
    );

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
        assert_eq!(
            parse_pci_id(&format!("{:x}", 0x1234)).unwrap(),
            parse_pci_id(&format!("{:X}", 0x1234)).unwrap(),
        );

        assert_eq!(
            parse_pci_id(&format!("{:#x}", 0x1234)).unwrap(),
            parse_pci_id(&format!("{:#X}", 0x1234)).unwrap(),
        );
        assert_eq!(
            parse_pci_id(&format!("{:#x}", 0x1234)).unwrap(),
            parse_pci_id(&format!("{:#X}", 0x1234)).unwrap(),
        );
    }
}
