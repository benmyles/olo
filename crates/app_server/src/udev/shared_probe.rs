//! `--probe-shared`: one try of a window's picture drawn by one GPU and
//! composed by another, the way the session does it (`udev::probe_shared`
//! has what it's for). Nothing here touches a display.

use std::path::Path;

use smithay::{
    backend::{
        allocator::{
            Allocator as _, Buffer as _, Fourcc, Modifier,
            dmabuf::{AsDmabuf as _, Dmabuf},
            gbm::{GbmAllocator, GbmBufferFlags, GbmDevice},
        },
        drm::{DrmDeviceFd, DrmNode},
        renderer::{
            Bind, Color32F, ExportMem as _, Frame as _, ImportDma, ImportMem as _, Renderer as _, TextureMapping as _,
            gles::GlesRenderer,
            multigpu::{GpuManager, MultiTexture, gbm::GbmGlesBackend},
        },
    },
    utils::{Physical, Rectangle, Size, Transform},
};

type Gpus = GpuManager<GbmGlesBackend<GlesRenderer, DrmDeviceFd>>;
type Card<'a> = (DrmNode, &'a GbmDevice<DrmDeviceFd>);

fn said(e: impl std::fmt::Display) -> String {
    e.to_string()
}

/// A picture no shuffling leaves looking right: red grows across, green
/// grows down, blue in squares of 16, a white line from corner to
/// corner and a red edge. RGBA, a byte each. `second`: another picture
/// (its colours the other way), for what is drawn into the same buffer
/// next.
fn pattern(w: u32, h: u32, second: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h {
        for x in 0..w {
            let edge = x < 3 || y < 3 || x >= w - 3 || y >= h - 3;
            let line = (i64::from(x) * i64::from(h) - i64::from(y) * i64::from(w)).abs() < i64::from(w) * 2;
            let [r, g, b] = if edge {
                [255, 0, 0]
            } else if line {
                [255, 255, 255]
            } else {
                [(x * 255 / (w - 1)) as u8, (y * 255 / (h - 1)) as u8, if (x / 16 + y / 16) % 2 == 0 { 255 } else { 0 }]
            };
            out.extend(if second { [255 - r, 255 - g, 255 - b, 255] } else { [r, g, b, 255] });
        }
    }
    out
}

/// How an app's buffer is made, for a try.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(super) enum Buffer {
    /// As the GPU it draws with likes best, of what the session offers.
    Its,
    /// Linear: what an app may choose when it's told the display is
    /// another GPU's (the only layout both know).
    Linear,
    /// Linear, and said to be for the composing GPU to sample (an app
    /// that follows the display's tranche: `set_sampling_device`).
    LinearForComposer,
}

/// The modifiers `node` can both draw into and take as a texture, for
/// this format: what a buffer made there may be laid out as.
fn layouts(gpus: &mut Gpus, node: DrmNode, code: Fourcc) -> Result<Vec<Modifier>, String> {
    let renderer = gpus.single_renderer(&node).map_err(said)?;
    let draws = Bind::<Dmabuf>::supported_formats(&renderer).ok_or("it says nothing of what it draws into")?;
    let takes = ImportDma::dmabuf_formats(&renderer);
    let both: Vec<Modifier> = draws.iter().filter(|f| f.code == code && f.modifier != Modifier::Invalid && takes.contains(f)).map(|f| f.modifier).collect();
    if both.is_empty() { Err(format!("no layout of {code:?} it both draws into and takes")) } else { Ok(both) }
}

/// The picture drawn on `from` into a buffer as an app's would be,
/// composed on `to` and read back; then another drawn into the same
/// buffer and composed again, and then a part of it (what a window does
/// at every frame). What differs, in words.
pub(super) fn once(gpus: &mut Gpus, from: Card<'_>, to: Card<'_>, w: u32, h: u32, how: Buffer, keep: Option<&Path>) -> Result<String, String> {
    let size: Size<i32, Physical> = (w as i32, h as i32).into();
    let all = [Rectangle::from_size(size)];
    let whole = Rectangle::from_size((f64::from(w), f64::from(h)).into());

    // An app's buffer: made on the GPU it draws with.
    let mut modifiers = layouts(gpus, from.0, Fourcc::Argb8888)?;
    if how != Buffer::Its {
        modifiers.retain(|m| *m == Modifier::Linear);
        if modifiers.is_empty() {
            return Err("the GPU that draws can't draw into a linear buffer".into());
        }
    }
    let mut buffer = GbmAllocator::new(from.1.clone(), GbmBufferFlags::RENDERING).create_buffer(w, h, Fourcc::Argb8888, &modifiers).map_err(said)?.export().map_err(said)?;
    let layout = format!("the app's buffer laid out {:?}, {} bytes a row", buffer.format().modifier, buffer.strides().next().unwrap_or(0));
    // (As the session marks a client's buffer: where it was made, or
    // where the client said it is for.)
    buffer.set_node(if how == Buffer::LinearForComposer { to.0 } else { from.0 });
    let modifiers = layouts(gpus, to.0, Fourcc::Abgr8888)?;
    let mut target = GbmAllocator::new(to.1.clone(), GbmBufferFlags::RENDERING).create_buffer(w, h, Fourcc::Abgr8888, &modifiers).map_err(said)?.export().map_err(said)?;

    // Three pictures, as a window's frames go: the first; another, all of
    // it new; then the first again in a part of it only, the rest left,
    // imported into the texture the frame before made with only that
    // part said to have changed (the copy kept for the other GPU is
    // drawn into again there, as at a window's every commit).
    let part: Rectangle<i32, Physical> = Rectangle::new(((w / 4) as i32, (h / 4) as i32).into(), (((w / 2) as i32).max(1), ((h / 2) as i32).max(1)).into());
    let inside = |i: usize| {
        let (x, y) = ((i as u32 % w) as i32, (i as u32 / w) as i32);
        x >= part.loc.x && x < part.loc.x + part.size.w && y >= part.loc.y && y < part.loc.y + part.size.h
    };
    let mut wrong = Vec::new();
    let mut kept: Option<MultiTexture> = None;
    for step in 0..3 {
        let second = step == 1;
        let drawn: Vec<u8> = if step == 2 {
            let (first, other) = (pattern(w, h, false), pattern(w, h, true));
            (0..(w * h) as usize).flat_map(|i| if inside(i) { first[i * 4..i * 4 + 4].to_vec() } else { other[i * 4..i * 4 + 4].to_vec() }).collect()
        } else {
            pattern(w, h, second)
        };
        {
            let mut renderer = gpus.single_renderer(&from.0).map_err(said)?;
            let texture = renderer.import_memory(&drawn, Fourcc::Abgr8888, (w as i32, h as i32).into(), false).map_err(said)?;
            let mut fb = renderer.bind(&mut buffer).map_err(said)?;
            let mut frame = renderer.render(&mut fb, size, Transform::Normal).map_err(said)?;
            frame.clear(Color32F::TRANSPARENT, &all).map_err(said)?;
            frame.render_texture_from_to(&texture, whole, all[0], &all, &[], Transform::Normal, 1.0).map_err(said)?;
            let _ = frame.finish().map_err(said)?.wait();
        }

        // As the session does when a client makes a buffer: imported
        // first by the renderer of the GPU apps draw with, whichever GPU
        // the buffer says it's for (refused there, the client is told
        // its buffer is no good).
        if step == 0 {
            gpus.single_renderer(&from.0).map_err(said)?.import_dmabuf(&buffer, None).map_err(|e| format!("the session's first import, on the GPU apps draw with: {e}"))?;
        }

        // Composed on the other GPU: imported as a window's buffer is,
        // drawn into a buffer of that GPU's own, and read back.
        let mut renderer = gpus.single_renderer(&to.0).map_err(said)?;
        let texture = match (&kept, step) {
            (Some(before), 2) => {
                let changed = [Rectangle::<i32, smithay::utils::Buffer>::new((part.loc.x, part.loc.y).into(), (part.size.w, part.size.h).into())];
                renderer.import_dmabuf_again(&buffer, before, Some(&changed)).map_err(|e| format!("the import again: {e}"))?
            }
            _ => renderer.import_dmabuf(&buffer, None).map_err(|e| format!("the import: {e}"))?,
        };
        kept = Some(texture.clone());
        let mut fb = renderer.bind(&mut target).map_err(said)?;
        {
            let mut frame = renderer.render(&mut fb, size, Transform::Normal).map_err(said)?;
            frame.clear(Color32F::BLACK, &all).map_err(said)?;
            frame.render_texture_from_to(&texture, whole, all[0], &all, &[], Transform::Normal, 1.0).map_err(said)?;
            let _ = frame.finish().map_err(said)?.wait();
        }
        let mapping = renderer.copy_framebuffer(&fb, Rectangle::from_size((w as i32, h as i32).into()), Fourcc::Abgr8888).map_err(said)?;
        let flipped = mapping.flipped();
        let pixels = renderer.map_texture(&mapping).map_err(said)?;
        let row = (w * 4) as usize;
        if pixels.len() < row * h as usize {
            return Err(format!("{} bytes read back, of {}", pixels.len(), row * h as usize));
        }
        // (Top row first. A buffer drawn into and then read as a texture
        // is the other way up from one read back: put right here, so a
        // picture composed rightly is the picture drawn.)
        let got: Vec<u8> = if flipped { pixels[..row * h as usize].to_vec() } else { pixels.chunks_exact(row).take(h as usize).rev().flatten().copied().collect() };

        // What differs (a shade or two is rounding, not shuffling).
        let differs = |i: usize| (0..4).any(|c| drawn[i * 4 + c].abs_diff(got[i * 4 + c]) > 2);
        let which = ["the first picture", "the second picture", "a part drawn again"][step];
        if let Some(keep) = keep {
            let name = keep.file_name().unwrap_or_default().to_string_lossy().into_owned();
            for (what, pixels) in [("drawn", &drawn), ("composed", &got)] {
                if let Some(picture) = image::RgbaImage::from_raw(w, h, pixels.clone()) {
                    let _ = picture.save(keep.with_file_name(format!("{name}-{}-{what}.png", step + 1)));
                }
            }
        }
        if let Some(first) = (0..(w * h) as usize).find(|&i| differs(i)) {
            let count = (0..(w * h) as usize).filter(|&i| differs(i)).count();
            // (The other picture, whole: what was there before, shown again.)
            let other = pattern(w, h, !second);
            let stale = second && (0..(w * h) as usize).all(|i| (0..4).all(|c| other[i * 4 + c].abs_diff(got[i * 4 + c]) <= 2));
            let (x, y) = (first as u32 % w, first as u32 / w);
            let at = |pixels: &[u8]| format!("{:?}", &pixels[first * 4..first * 4 + 4]);
            wrong.push(if stale {
                format!("{which} never showed: the first was still there")
            } else {
                format!("{which}: {count} of {} pixels differ, the first at {x}, {y} (drawn {}, composed {})", w * h, at(&drawn), at(&got))
            });
        }
    }
    Ok(if wrong.is_empty() { format!("the same, all three pictures ({layout})") } else { format!("WRONG: {} ({layout})", wrong.join("; ")) })
}
