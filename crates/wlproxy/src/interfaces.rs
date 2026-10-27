//! The globals the proxy understands. Programs see only these: a message
//! to an interface the proxy can't decode couldn't be passed on, let alone
//! replayed.

use std::sync::OnceLock;

use wayland_backend::protocol::Interface;
use wayland_client::{Proxy, protocol};
use wayland_protocols::{wp, xdg};

pub fn globals() -> &'static [&'static Interface] {
    static LIST: OnceLock<Vec<&'static Interface>> = OnceLock::new();
    LIST.get_or_init(|| {
        vec![
            protocol::wl_compositor::WlCompositor::interface(),
            protocol::wl_subcompositor::WlSubcompositor::interface(),
            protocol::wl_shm::WlShm::interface(),
            protocol::wl_seat::WlSeat::interface(),
            protocol::wl_output::WlOutput::interface(),
            protocol::wl_data_device_manager::WlDataDeviceManager::interface(),
            protocol::wl_fixes::WlFixes::interface(),
            xdg::shell::client::xdg_wm_base::XdgWmBase::interface(),
            xdg::decoration::zv1::client::zxdg_decoration_manager_v1::ZxdgDecorationManagerV1::interface(),
            xdg::xdg_output::zv1::client::zxdg_output_manager_v1::ZxdgOutputManagerV1::interface(),
            xdg::activation::v1::client::xdg_activation_v1::XdgActivationV1::interface(),
            xdg::foreign::zv2::client::zxdg_exporter_v2::ZxdgExporterV2::interface(),
            xdg::foreign::zv2::client::zxdg_importer_v2::ZxdgImporterV2::interface(),
            wp::linux_dmabuf::zv1::client::zwp_linux_dmabuf_v1::ZwpLinuxDmabufV1::interface(),
            wp::viewporter::client::wp_viewporter::WpViewporter::interface(),
            wp::fractional_scale::v1::client::wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1::interface(),
            wp::text_input::zv3::client::zwp_text_input_manager_v3::ZwpTextInputManagerV3::interface(),
            wp::pointer_constraints::zv1::client::zwp_pointer_constraints_v1::ZwpPointerConstraintsV1::interface(),
            wp::relative_pointer::zv1::client::zwp_relative_pointer_manager_v1::ZwpRelativePointerManagerV1::interface(),
            wp::pointer_gestures::zv1::client::zwp_pointer_gestures_v1::ZwpPointerGesturesV1::interface(),
            wp::presentation_time::client::wp_presentation::WpPresentation::interface(),
            wp::idle_inhibit::zv1::client::zwp_idle_inhibit_manager_v1::ZwpIdleInhibitManagerV1::interface(),
            wp::cursor_shape::v1::client::wp_cursor_shape_manager_v1::WpCursorShapeManagerV1::interface(),
            wp::single_pixel_buffer::v1::client::wp_single_pixel_buffer_manager_v1::WpSinglePixelBufferManagerV1::interface(),
            wp::primary_selection::zv1::client::zwp_primary_selection_device_manager_v1::ZwpPrimarySelectionDeviceManagerV1::interface(),
            wp::keyboard_shortcuts_inhibit::zv1::client::zwp_keyboard_shortcuts_inhibit_manager_v1::ZwpKeyboardShortcutsInhibitManagerV1::interface(),
            wp::fifo::v1::client::wp_fifo_manager_v1::WpFifoManagerV1::interface(),
            wp::commit_timing::v1::client::wp_commit_timing_manager_v1::WpCommitTimingManagerV1::interface(),
            wayland_protocols_wlr::layer_shell::v1::client::zwlr_layer_shell_v1::ZwlrLayerShellV1::interface(),
        ]
    })
}

pub fn global(name: &[u8]) -> Option<&'static Interface> {
    globals().iter().copied().find(|i| i.name.as_bytes() == name)
}

pub fn display() -> &'static Interface {
    protocol::wl_display::WlDisplay::interface()
}

/// The opcode of `iface`'s request (or event) called `name`.
pub fn request(iface: &Interface, name: &str) -> u16 {
    iface.requests.iter().position(|m| m.name == name).expect("known request") as u16
}

pub fn event(iface: &Interface, name: &str) -> u16 {
    iface.events.iter().position(|m| m.name == name).expect("known event") as u16
}
