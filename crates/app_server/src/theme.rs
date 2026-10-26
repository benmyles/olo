//! The palette app_server currently draws with. Changing it bumps a
//! generation counter so cached decorations know to redraw.

use std::sync::{
    Arc, RwLock,
    atomic::{AtomicU64, Ordering},
};

use tabula_theme::{Palette, ThemeMode};

static PALETTE: RwLock<Option<Arc<Palette>>> = RwLock::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(1);

pub fn palette() -> Arc<Palette> {
    if let Some(p) = PALETTE.read().unwrap().as_ref() {
        return p.clone();
    }
    let mut slot = PALETTE.write().unwrap();
    slot.get_or_insert_with(|| Arc::new(Palette::light())).clone()
}

pub fn mode() -> ThemeMode {
    palette().mode
}

/// Switch light/dark. Returns true if anything changed.
pub fn set_mode(mode: ThemeMode) -> bool {
    if palette().mode == mode {
        return false;
    }
    *PALETTE.write().unwrap() = Some(Arc::new(Palette::for_mode(mode)));
    GENERATION.fetch_add(1, Ordering::Relaxed);
    true
}

pub fn generation() -> u64 {
    GENERATION.load(Ordering::Relaxed)
}

/// sRGB bytes to the premultiplied float colour smithay wants.
pub fn to_f32(c: tabula_theme::Rgba) -> [f32; 4] {
    let a = c[3] as f32 / 255.0;
    [
        c[0] as f32 / 255.0 * a,
        c[1] as f32 / 255.0 * a,
        c[2] as f32 / 255.0 * a,
        a,
    ]
}
