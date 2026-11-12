//! Olo: a window with no display, for drawing a view where nothing shows
//! it: the layout lint's tests (`olo_kit::lint`) lay a window out and
//! record what it draws, with Olo's text system and fonts, at any scale.
//! It has a size and a scale and says so as a compositor would (a resize
//! asked for is given at once); nothing is rasterised or shown. Its atlas
//! hands out tiles of the right sizes without keeping their pixels.

use std::{
    cell::RefCell,
    collections::HashMap,
    rc::Rc,
    sync::{Arc, Mutex},
};

use raw_window_handle as rwh;

use crate::{
    AtlasKey, AtlasTextureId, AtlasTextureKind, AtlasTile, Bounds, Capslock, DevicePixels, GpuSpecs, Modifiers, Pixels, PlatformAtlas, PlatformDisplay, PlatformInput, PlatformWindow, Point,
    RequestFrameOptions, Scene, Size, TileId, WindowAppearance, WindowBackgroundAppearance, WindowBounds, WindowParams,
};

#[derive(Default)]
struct Callbacks {
    input: Option<Box<dyn FnMut(PlatformInput)>>,
    resize: Option<Box<dyn FnMut(Size<Pixels>, f32)>>,
    close: Option<Box<dyn FnOnce()>>,
}

struct State {
    size: Size<Pixels>,
    scale: f32,
    title: String,
    callbacks: Callbacks,
}

pub(crate) struct HeadlessWindow {
    state: Rc<RefCell<State>>,
    atlas: Arc<HeadlessAtlas>,
}

impl HeadlessWindow {
    pub(crate) fn new(params: WindowParams) -> HeadlessWindow {
        HeadlessWindow {
            state: Rc::new(RefCell::new(State { size: params.bounds.size, scale: 1.0, title: String::new(), callbacks: Callbacks::default() })),
            atlas: Arc::new(HeadlessAtlas::default()),
        }
    }

    /// Says its size and scale as a compositor would.
    fn tell_size(&self) {
        let (size, scale, callback) = {
            let mut state = self.state.borrow_mut();
            (state.size, state.scale, state.callbacks.resize.take())
        };
        if let Some(mut callback) = callback {
            callback(size, scale);
            self.state.borrow_mut().callbacks.resize.get_or_insert(callback);
        }
    }
}

impl rwh::HasWindowHandle for HeadlessWindow {
    fn window_handle(&self) -> Result<rwh::WindowHandle<'_>, rwh::HandleError> {
        Err(rwh::HandleError::Unavailable)
    }
}

impl rwh::HasDisplayHandle for HeadlessWindow {
    fn display_handle(&self) -> Result<rwh::DisplayHandle<'_>, rwh::HandleError> {
        Err(rwh::HandleError::Unavailable)
    }
}

impl PlatformWindow for HeadlessWindow {
    fn bounds(&self) -> Bounds<Pixels> {
        Bounds { origin: Point::default(), size: self.state.borrow().size }
    }

    fn is_maximized(&self) -> bool {
        false
    }

    fn window_bounds(&self) -> WindowBounds {
        WindowBounds::Windowed(self.bounds())
    }

    fn content_size(&self) -> Size<Pixels> {
        self.state.borrow().size
    }

    /// (Given at once, as asked: a window that sizes itself to what it
    /// shows gets that size.)
    fn resize(&mut self, size: Size<Pixels>) {
        self.state.borrow_mut().size = size;
        self.tell_size();
    }

    fn scale_factor(&self) -> f32 {
        self.state.borrow().scale
    }

    fn appearance(&self) -> WindowAppearance {
        WindowAppearance::Light
    }

    fn display(&self) -> Option<Rc<dyn PlatformDisplay>> {
        None
    }

    fn mouse_position(&self) -> Point<Pixels> {
        Point::default()
    }

    fn modifiers(&self) -> Modifiers {
        Modifiers::default()
    }

    fn capslock(&self) -> Capslock {
        Capslock::default()
    }

    fn activate(&self) {}

    fn is_active(&self) -> bool {
        true
    }

    fn is_hovered(&self) -> bool {
        false
    }

    fn set_title(&mut self, title: &str) {
        self.state.borrow_mut().title = title.to_owned();
    }

    fn set_background_appearance(&self, _: WindowBackgroundAppearance) {}

    fn minimize(&self) {}

    fn zoom(&self) {}

    fn toggle_fullscreen(&self) {}

    fn is_fullscreen(&self) -> bool {
        false
    }

    fn on_request_frame(&self, _: Box<dyn FnMut(RequestFrameOptions) -> bool>) {}

    fn on_input(&self, callback: Box<dyn FnMut(PlatformInput)>) {
        self.state.borrow_mut().callbacks.input = Some(callback);
    }

    fn on_active_status_change(&self, _: Box<dyn FnMut(bool)>) {}

    fn on_hover_status_change(&self, _: Box<dyn FnMut(bool)>) {}

    fn on_resize(&self, callback: Box<dyn FnMut(Size<Pixels>, f32)>) {
        self.state.borrow_mut().callbacks.resize = Some(callback);
    }

    fn on_moved(&self, _: Box<dyn FnMut()>) {}

    fn on_should_close(&self, _: Box<dyn FnMut() -> bool>) {}

    fn on_close(&self, callback: Box<dyn FnOnce()>) {
        self.state.borrow_mut().callbacks.close = Some(callback);
    }

    fn on_appearance_changed(&self, _: Box<dyn FnMut()>) {}

    fn draw(&self, _: &Scene) {}

    fn sprite_atlas(&self) -> Arc<dyn PlatformAtlas> {
        self.atlas.clone()
    }

    fn get_title(&self) -> String {
        self.state.borrow().title.clone()
    }

    fn gpu_specs(&self) -> Option<GpuSpecs> {
        None
    }

    fn update_ime_position(&self, _: Bounds<Pixels>) {}

    fn set_headless_scale(&self, scale: f32) {
        self.state.borrow_mut().scale = scale;
        self.tell_size();
    }

    fn headless_input(&self, input: PlatformInput) {
        let callback = self.state.borrow_mut().callbacks.input.take();
        if let Some(mut callback) = callback {
            callback(input);
            self.state.borrow_mut().callbacks.input.get_or_insert(callback);
        }
    }
}

impl Drop for HeadlessWindow {
    fn drop(&mut self) {
        let close = self.state.borrow_mut().callbacks.close.take();
        if let Some(close) = close {
            close();
        }
    }
}

/// Tiles of the sizes asked for, none of their pixels kept.
#[derive(Default)]
struct HeadlessAtlas(Mutex<(u32, HashMap<AtlasKey, AtlasTile>)>);

impl PlatformAtlas for HeadlessAtlas {
    fn get_or_insert_with<'a>(&self, key: &AtlasKey, build: &mut dyn FnMut() -> anyhow::Result<Option<(Size<DevicePixels>, std::borrow::Cow<'a, [u8]>)>>) -> anyhow::Result<Option<AtlasTile>> {
        if let Some(tile) = self.0.lock().unwrap_or_else(|e| e.into_inner()).1.get(key) {
            return Ok(Some(tile.clone()));
        }
        let Some((size, _)) = build()? else { return Ok(None) };
        let mut state = self.0.lock().unwrap_or_else(|e| e.into_inner());
        state.0 += 2;
        let tile = AtlasTile {
            texture_id: AtlasTextureId { index: state.0 - 1, kind: AtlasTextureKind::Monochrome },
            tile_id: TileId(state.0),
            padding: 0,
            bounds: Bounds { origin: Point::default(), size },
        };
        state.1.insert(key.clone(), tile.clone());
        Ok(Some(tile))
    }

    fn remove(&self, key: &AtlasKey) {
        self.0.lock().unwrap_or_else(|e| e.into_inner()).1.remove(key);
    }
}
