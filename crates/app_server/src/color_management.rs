//! `wp_color_management_v1`: programs say what colours their windows
//! hold (an image description: primaries, transfer function, light), and
//! learn each display's. A surface's description is cached state
//! (Smithay's `SurfaceColor`), applied with its next commit; the renderer
//! converts its buffers from it into the display's colours as they're
//! drawn (`gles::color`), so an HDR video (PQ in BT.2020) shines on an
//! HDR display and is tone-mapped onto an SDR one, and a Display P3 photo
//! keeps its colours on a wide display. Parametric descriptions only (no
//! ICC files): the named transfer functions the renderer has (gamma 2.2,
//! sRGB, linear, PQ) and any primaries.

use std::{
    cell::RefCell,
    sync::{
        Mutex,
        atomic::{AtomicBool, Ordering},
    },
};

use olo_color::{Chromaticity, Primaries, Transfer};
use smithay::{
    backend::renderer::{ColorSource, SurfaceColor},
    output::Output,
    reexports::{
        wayland_protocols::wp::color_management::v1::server::{
            wp_color_management_output_v1::{self, WpColorManagementOutputV1},
            wp_color_management_surface_feedback_v1::{self, WpColorManagementSurfaceFeedbackV1},
            wp_color_management_surface_v1::{self, WpColorManagementSurfaceV1},
            wp_color_manager_v1::{self, Feature, RenderIntent, TransferFunction, WpColorManagerV1},
            wp_image_description_creator_icc_v1::{self, WpImageDescriptionCreatorIccV1},
            wp_image_description_creator_params_v1::{self, WpImageDescriptionCreatorParamsV1},
            wp_image_description_info_v1::WpImageDescriptionInfoV1,
            wp_image_description_v1::{self, WpImageDescriptionV1},
        },
        wayland_server::{Client, DataInit, DisplayHandle, New, Resource, Weak, protocol::wl_surface::WlSurface},
    },
    wayland::{Dispatch2, GlobalData, GlobalDispatch2, compositor::with_states},
};

use crate::state::{AnvilState, Backend};

/// The named primaries offered, as the protocol numbers them.
const PRIMARIES: [(u32, Primaries); 5] =
    [(1, Primaries::SRGB), (6, Primaries::BT2020), (8, Primaries { white: Chromaticity { x: 0.314, y: 0.351 }, ..Primaries::DISPLAY_P3 }), (9, Primaries::DISPLAY_P3), (10, Primaries::ADOBE_RGB)];
const TF_GAMMA22: u32 = 2;
const TF_EXT_LINEAR: u32 = 5;
const TF_SRGB: u32 = 9;
const TF_PQ: u32 = 11;
const TFS: [u32; 4] = [TF_GAMMA22, TF_SRGB, TF_EXT_LINEAR, TF_PQ];

/// What colours something holds: a surface's buffers, or a display.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Description {
    pub primaries: Primaries,
    /// The protocol's name for them, if they're named.
    pub named: Option<u32>,
    pub tf: u32,
    /// cd/m²: black, the most, and the reference white.
    pub min_lum: f64,
    pub max_lum: f64,
    pub reference_lum: f64,
    /// A display's own: its primaries and light (the volume shown).
    pub target: Option<(Primaries, f64, f64)>,
    pub max_cll: Option<u32>,
    pub max_fall: Option<u32>,
}

impl Description {
    /// sRGB, as everything is unless it says.
    pub fn srgb() -> Description {
        Description { primaries: Primaries::SRGB, named: Some(1), tf: TF_GAMMA22, min_lum: 0.2, max_lum: 80.0, reference_lum: 80.0, target: None, max_cll: None, max_fall: None }
    }

    /// What the renderer converts its buffers from (None: sRGB, nothing).
    pub fn source(&self) -> Option<ColorSource> {
        let transfer = match self.tf {
            TF_PQ => Transfer::Pq,
            TF_EXT_LINEAR => Transfer::Linear,
            // (sRGB's own curve is taken as gamma 2.2, as a display shows
            // it, the same as what says nothing.)
            _ => Transfer::Gamma22,
        };
        let source = ColorSource { primaries: self.primaries, transfer };
        (source != ColorSource::SRGB).then_some(source)
    }

    /// The same description, the same number, for as long as app_server
    /// runs (the protocol's identity).
    fn identity(&self) -> u32 {
        KNOWN.with(|k| {
            let mut k = k.borrow_mut();
            match k.iter().position(|d| d == self) {
                Some(i) => i as u32 + 1,
                None => {
                    k.push(*self);
                    k.len() as u32
                }
            }
        })
    }
}

thread_local! {
    /// Every description seen, by identity (index + 1).
    static KNOWN: RefCell<Vec<Description>> = const { RefCell::new(Vec::new()) };
    /// The displays' and surfaces' objects, to tell of changes.
    static OUTPUTS: RefCell<Vec<WpColorManagementOutputV1>> = const { RefCell::new(Vec::new()) };
    static FEEDBACKS: RefCell<Vec<(WpColorManagementSurfaceFeedbackV1, Weak<WlSurface>)>> = const { RefCell::new(Vec::new()) };
}

/// A display's description: how app_server draws for it (`color`).
pub fn of_output(output: &Output) -> Description {
    let Some(c) = crate::color::of(output) else { return Description::srgb() };
    let own = c.display.primaries().unwrap_or(Primaries::SRGB);
    let applied = crate::color::applied(output);
    let peak = f64::from(crate::color::peak_nits(&c.display));
    let black = c.display.black_nits.unwrap_or(0.0005);
    match crate::color::drawing(output) {
        crate::color::Drawing::Hdr { .. } => Description {
            primaries: Primaries::BT2020,
            named: Some(6),
            tf: TF_PQ,
            min_lum: 0.005,
            max_lum: 10_000.0,
            reference_lum: f64::from(applied.sdr_nits),
            target: Some((own, black, peak)),
            max_cll: Some(peak as u32),
            max_fall: Some(c.display.full_nits.unwrap_or(peak) as u32),
        },
        _ => Description {
            primaries: own,
            named: PRIMARIES.iter().find(|(_, p)| *p == own).map(|(n, _)| *n),
            target: Some((own, black, c.display.full_nits.unwrap_or(80.0).max(80.0))),
            ..Description::srgb()
        },
    }
}

/// Make the global (`AnvilState::init`).
pub fn init<B: Backend + 'static>(dh: &DisplayHandle) {
    dh.create_global::<AnvilState<B>, WpColorManagerV1, _>(1, GlobalData);
}

/// `output`'s colours changed: its objects, and surfaces' feedback, told.
pub fn output_changed(output: &Output) {
    OUTPUTS.with(|o| {
        o.borrow_mut().retain(|r| r.is_alive());
        for r in o.borrow().iter() {
            if r.data::<CmOutput>().is_some_and(|d| d.0.as_ref() == Some(output)) {
                r.image_description_changed();
            }
        }
    });
    FEEDBACKS.with(|f| {
        f.borrow_mut().retain(|(r, s)| r.is_alive() && s.upgrade().is_ok());
        for (r, s) in f.borrow().iter() {
            if let Ok(surface) = s.upgrade() {
                r.preferred_changed(preferred(&surface).identity());
            }
        }
    });
}

/// What a surface had best draw in: the description of the display it's
/// mostly on (sRGB before it shows).
fn preferred(surface: &WlSurface) -> Description {
    let output = with_states(surface, |states| smithay::desktop::utils::surface_primary_scanout_output(surface, states));
    output.map(|o| of_output(&o)).unwrap_or_else(Description::srgb)
}

/// A display's object's data: the display.
pub struct CmOutput(Option<Output>);
/// A surface's object's data.
pub struct CmSurface(Weak<WlSurface>);
/// A feedback object's data.
pub struct CmFeedback(Weak<WlSurface>);
/// An image description's: None when it failed.
pub struct CmDescription(Option<Description>);
/// The parametric creator's: what's set so far.
#[derive(Default)]
pub struct CmParams(Mutex<Params>);
#[derive(Default, Clone, Copy)]
pub struct Params {
    tf: Option<u32>,
    primaries: Option<(Primaries, Option<u32>)>,
    luminances: Option<(f64, f64, f64)>,
    mastering: bool,
    mastering_lum: bool,
    max_cll: Option<u32>,
    max_fall: Option<u32>,
}
/// The ICC creator's (never offered: refused).
pub struct CmIcc;

/// Marks a surface that has its colour object (one each).
struct HasColorObject(AtomicBool);

impl<B: Backend + 'static> GlobalDispatch2<WpColorManagerV1, AnvilState<B>> for GlobalData {
    fn bind(&self, _state: &mut AnvilState<B>, _handle: &DisplayHandle, _client: &Client, resource: New<WpColorManagerV1>, data_init: &mut DataInit<'_, AnvilState<B>>) {
        let manager = data_init.init(resource, GlobalData);
        manager.supported_intent(RenderIntent::Perceptual);
        manager.supported_intent(RenderIntent::Relative);
        for f in [Feature::Parametric, Feature::SetPrimaries, Feature::SetLuminances, Feature::SetMasteringDisplayPrimaries] {
            manager.supported_feature(f);
        }
        for tf in TFS {
            if let Ok(tf) = TransferFunction::try_from(tf) {
                manager.supported_tf_named(tf);
            }
        }
        for (n, _) in PRIMARIES {
            if let Ok(p) = wp_color_manager_v1::Primaries::try_from(n) {
                manager.supported_primaries_named(p);
            }
        }
        manager.done();
    }
}

impl<B: Backend + 'static> Dispatch2<WpColorManagerV1, AnvilState<B>> for GlobalData {
    fn request(
        &self,
        _state: &mut AnvilState<B>,
        _client: &Client,
        resource: &WpColorManagerV1,
        request: wp_color_manager_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, AnvilState<B>>,
    ) {
        use wp_color_manager_v1::Request;
        match request {
            Request::GetOutput { id, output } => {
                let o = data_init.init(id, CmOutput(Output::from_resource(&output)));
                OUTPUTS.with(|v| v.borrow_mut().push(o));
            }
            Request::GetSurface { id, surface } => {
                with_states(&surface, |s| s.data_map.insert_if_missing_threadsafe(|| HasColorObject(AtomicBool::new(false))));
                let exists = with_states(&surface, |s| s.data_map.get::<HasColorObject>().is_some_and(|h| h.0.swap(true, Ordering::SeqCst)));
                data_init.init(id, CmSurface(surface.downgrade()));
                if exists {
                    resource.post_error(wp_color_manager_v1::Error::SurfaceExists, "the surface has a colour management object already");
                }
            }
            Request::GetSurfaceFeedback { id, surface } => {
                let f = data_init.init(id, CmFeedback(surface.downgrade()));
                FEEDBACKS.with(|v| v.borrow_mut().push((f, surface.downgrade())));
            }
            Request::CreateIccCreator { obj } => {
                data_init.init(obj, CmIcc);
                resource.post_error(wp_color_manager_v1::Error::UnsupportedFeature, "ICC files aren't taken: describe colours parametrically");
            }
            Request::CreateParametricCreator { obj } => {
                data_init.init(obj, CmParams::default());
            }
            Request::CreateWindowsScrgb { image_description } => {
                data_init.init(image_description, CmDescription(None));
                resource.post_error(wp_color_manager_v1::Error::UnsupportedFeature, "scRGB isn't offered");
            }
            Request::Destroy => {}
            _ => {}
        }
    }
}

impl<B: Backend + 'static> Dispatch2<WpColorManagementOutputV1, AnvilState<B>> for CmOutput {
    fn request(
        &self,
        _state: &mut AnvilState<B>,
        _client: &Client,
        _resource: &WpColorManagementOutputV1,
        request: wp_color_management_output_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, AnvilState<B>>,
    ) {
        if let wp_color_management_output_v1::Request::GetImageDescription { image_description } = request {
            let d = self.0.as_ref().map(of_output).unwrap_or_else(Description::srgb);
            let r = data_init.init(image_description, CmDescription(Some(d)));
            r.ready(d.identity());
        }
    }
}

impl<B: Backend + 'static> Dispatch2<WpColorManagementSurfaceV1, AnvilState<B>> for CmSurface {
    fn request(
        &self,
        _state: &mut AnvilState<B>,
        _client: &Client,
        resource: &WpColorManagementSurfaceV1,
        request: wp_color_management_surface_v1::Request,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, AnvilState<B>>,
    ) {
        use wp_color_management_surface_v1::Request;
        let Ok(surface) = self.0.upgrade() else {
            resource.post_error(wp_color_management_surface_v1::Error::Inert, "the surface is gone");
            return;
        };
        let pending = |source: Option<ColorSource>| with_states(&surface, |s| s.cached_state.get::<SurfaceColor>().pending().0 = source);
        match request {
            Request::SetImageDescription { image_description, render_intent } => {
                let intent_ok = matches!(render_intent.into_result(), Ok(RenderIntent::Perceptual | RenderIntent::Relative));
                if !intent_ok {
                    resource.post_error(wp_color_management_surface_v1::Error::RenderIntent, "that render intent isn't offered");
                    return;
                }
                match image_description.data::<CmDescription>().and_then(|d| d.0) {
                    Some(d) => pending(d.source()),
                    None => resource.post_error(wp_color_management_surface_v1::Error::ImageDescription, "that image description isn't ready"),
                }
            }
            Request::UnsetImageDescription => pending(None),
            Request::Destroy => pending(None),
            _ => {}
        }
    }

    fn destroyed(&self, _state: &mut AnvilState<B>, _client: smithay::reexports::wayland_server::backend::ClientId, _resource: &WpColorManagementSurfaceV1) {
        if let Ok(surface) = self.0.upgrade() {
            with_states(&surface, |s| {
                if let Some(h) = s.data_map.get::<HasColorObject>() {
                    h.0.store(false, Ordering::SeqCst);
                }
            });
        }
    }
}

impl<B: Backend + 'static> Dispatch2<WpColorManagementSurfaceFeedbackV1, AnvilState<B>> for CmFeedback {
    fn request(
        &self,
        _state: &mut AnvilState<B>,
        _client: &Client,
        resource: &WpColorManagementSurfaceFeedbackV1,
        request: wp_color_management_surface_feedback_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, AnvilState<B>>,
    ) {
        use wp_color_management_surface_feedback_v1::Request;
        let id = match request {
            Request::GetPreferred { image_description } | Request::GetPreferredParametric { image_description } => image_description,
            _ => return,
        };
        let Ok(surface) = self.0.upgrade() else {
            data_init.init(id, CmDescription(None));
            resource.post_error(wp_color_management_surface_feedback_v1::Error::Inert, "the surface is gone");
            return;
        };
        let d = preferred(&surface);
        let r = data_init.init(id, CmDescription(Some(d)));
        r.ready(d.identity());
    }
}

impl<B: Backend + 'static> Dispatch2<WpImageDescriptionCreatorIccV1, AnvilState<B>> for CmIcc {
    fn request(
        &self,
        _state: &mut AnvilState<B>,
        _client: &Client,
        _resource: &WpImageDescriptionCreatorIccV1,
        request: wp_image_description_creator_icc_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, AnvilState<B>>,
    ) {
        if let wp_image_description_creator_icc_v1::Request::Create { image_description } = request {
            let r = data_init.init(image_description, CmDescription(None));
            r.failed(wp_image_description_v1::Cause::Unsupported, "ICC files aren't taken".into());
        }
    }
}

impl<B: Backend + 'static> Dispatch2<WpImageDescriptionCreatorParamsV1, AnvilState<B>> for CmParams {
    fn request(
        &self,
        _state: &mut AnvilState<B>,
        _client: &Client,
        resource: &WpImageDescriptionCreatorParamsV1,
        request: wp_image_description_creator_params_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, AnvilState<B>>,
    ) {
        use wp_image_description_creator_params_v1::{Error, Request};
        let mut p = self.0.lock().unwrap_or_else(|e| e.into_inner());
        let xy = |x: i32, y: i32| Chromaticity { x: f64::from(x) / 1_000_000.0, y: f64::from(y) / 1_000_000.0 };
        match request {
            Request::SetTfNamed { tf } => {
                let tf = u32::from(tf);
                if p.tf.is_some() {
                    resource.post_error(Error::AlreadySet, "the transfer function is set already");
                } else if !TFS.contains(&tf) {
                    resource.post_error(Error::InvalidTf, "that transfer function isn't offered");
                } else {
                    p.tf = Some(tf);
                }
            }
            Request::SetTfPower { .. } => resource.post_error(Error::UnsupportedFeature, "power-law transfer functions aren't offered"),
            Request::SetPrimariesNamed { primaries } => {
                let n = u32::from(primaries);
                if p.primaries.is_some() {
                    resource.post_error(Error::AlreadySet, "the primaries are set already");
                } else if let Some((_, prim)) = PRIMARIES.iter().find(|(k, _)| *k == n) {
                    p.primaries = Some((*prim, Some(n)));
                } else {
                    resource.post_error(Error::InvalidPrimariesNamed, "those primaries aren't offered");
                }
            }
            Request::SetPrimaries { r_x, r_y, g_x, g_y, b_x, b_y, w_x, w_y } => {
                if p.primaries.is_some() {
                    resource.post_error(Error::AlreadySet, "the primaries are set already");
                } else {
                    p.primaries = Some((Primaries { red: xy(r_x, r_y), green: xy(g_x, g_y), blue: xy(b_x, b_y), white: xy(w_x, w_y) }, None));
                }
            }
            Request::SetLuminances { min_lum, max_lum, reference_lum } => {
                let (min, max, reference) = (f64::from(min_lum) / 10_000.0, f64::from(max_lum), f64::from(reference_lum));
                if p.luminances.is_some() {
                    resource.post_error(Error::AlreadySet, "the luminances are set already");
                } else if max <= min || reference <= min {
                    resource.post_error(Error::InvalidLuminance, "the luminances are out of order");
                } else {
                    p.luminances = Some((min, max, reference));
                }
            }
            // (Mastering metadata: taken, not used; Olo maps by the
            // display's own light.)
            Request::SetMasteringDisplayPrimaries { .. } => p.mastering = true,
            Request::SetMasteringLuminance { .. } => p.mastering_lum = true,
            Request::SetMaxCll { max_cll } => p.max_cll = Some(max_cll),
            Request::SetMaxFall { max_fall } => p.max_fall = Some(max_fall),
            Request::Create { image_description } => {
                let (Some(tf), Some((primaries, named))) = (p.tf, p.primaries) else {
                    data_init.init(image_description, CmDescription(None));
                    resource.post_error(Error::IncompleteSet, "a transfer function and primaries are needed");
                    return;
                };
                let (min_lum, max_lum, reference_lum) = p.luminances.unwrap_or(if tf == TF_PQ { (0.005, 10_000.0, 203.0) } else { (0.2, 80.0, 80.0) });
                let d = Description { primaries, named, tf, min_lum, max_lum, reference_lum, target: None, max_cll: p.max_cll, max_fall: p.max_fall };
                let r = data_init.init(image_description, CmDescription(primaries.plausible().then_some(d)));
                if primaries.plausible() {
                    r.ready(d.identity());
                } else {
                    r.failed(wp_image_description_v1::Cause::Unsupported, "those primaries aren't colours".into());
                }
            }
            _ => {}
        }
    }
}

impl<B: Backend + 'static> Dispatch2<WpImageDescriptionV1, AnvilState<B>> for CmDescription {
    fn request(
        &self,
        state: &mut AnvilState<B>,
        _client: &Client,
        resource: &WpImageDescriptionV1,
        request: wp_image_description_v1::Request,
        _dh: &DisplayHandle,
        data_init: &mut DataInit<'_, AnvilState<B>>,
    ) {
        if let wp_image_description_v1::Request::GetInformation { information } = request {
            let info = data_init.init(information, CmInfo);
            let Some(d) = self.0 else {
                resource.post_error(wp_image_description_v1::Error::NotReady, "it failed");
                return;
            };
            // (Sent once this request is done: `done` destroys the object,
            // and destroying one in the request that made it crashed
            // libwayland-server.)
            state.handle.insert_idle(move |_| send_information(&info, &d));
        }
    }
}

/// An image description's information, ending with `done`.
fn send_information(info: &WpImageDescriptionInfoV1, d: &Description) {
    if !info.is_alive() {
        return;
    }
    let m = |c: Chromaticity| ((c.x * 1_000_000.0).round() as i32, (c.y * 1_000_000.0).round() as i32);
    let corners = |p: &Primaries| {
        let (r, g, b, w) = (m(p.red), m(p.green), m(p.blue), m(p.white));
        [r.0, r.1, g.0, g.1, b.0, b.1, w.0, w.1]
    };
    match d.named.and_then(|n| wp_color_manager_v1::Primaries::try_from(n).ok()) {
        Some(n) => info.primaries_named(n),
        None => {
            let c = corners(&d.primaries);
            info.primaries(c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]);
        }
    }
    if let Ok(tf) = TransferFunction::try_from(d.tf) {
        info.tf_named(tf);
    }
    info.luminances((d.min_lum * 10_000.0).round() as u32, d.max_lum.round() as u32, d.reference_lum.round() as u32);
    let (target, tmin, tmax) = d.target.unwrap_or((d.primaries, d.min_lum, d.max_lum));
    let c = corners(&target);
    info.target_primaries(c[0], c[1], c[2], c[3], c[4], c[5], c[6], c[7]);
    info.target_luminance((tmin * 10_000.0).round() as u32, tmax.round() as u32);
    if let Some(c) = d.max_cll {
        info.target_max_cll(c);
    }
    if let Some(f) = d.max_fall {
        info.target_max_fall(f);
    }
    info.done();
}

/// An information object's (it has no requests).
pub struct CmInfo;

impl<B: Backend + 'static> Dispatch2<WpImageDescriptionInfoV1, AnvilState<B>> for CmInfo {
    fn request(
        &self,
        _state: &mut AnvilState<B>,
        _client: &Client,
        _resource: &WpImageDescriptionInfoV1,
        _request: <WpImageDescriptionInfoV1 as Resource>::Request,
        _dh: &DisplayHandle,
        _data_init: &mut DataInit<'_, AnvilState<B>>,
    ) {
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn descriptions_and_what_they_convert_from() {
        assert_eq!(Description::srgb().source(), None);
        let hdr10 = Description { primaries: Primaries::BT2020, named: Some(6), tf: TF_PQ, ..Description::srgb() };
        assert_eq!(hdr10.source(), Some(ColorSource { primaries: Primaries::BT2020, transfer: Transfer::Pq }));
        let p3 = Description { primaries: Primaries::DISPLAY_P3, named: Some(9), tf: TF_SRGB, ..Description::srgb() };
        assert_eq!(p3.source(), Some(ColorSource { primaries: Primaries::DISPLAY_P3, transfer: Transfer::Gamma22 }));
        // One number per description, the same each time.
        let (a, b) = (hdr10.identity(), p3.identity());
        assert!(a != b && a == hdr10.identity() && a > 0);
    }
}
