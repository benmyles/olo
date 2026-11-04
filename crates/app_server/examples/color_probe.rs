//! A Wayland client for trying app_server's colour management
//! (`wp_color_management_v1`): prints what the compositor offers and each
//! display's description, then shows a 256 × 256 window whose buffer is
//! tagged PQ in BT.2020 and filled with PQ's code for 100 cd/m² (130 of
//! 255). Drawn as the tag says, SDR's white being 203 cd/m², it's a grey
//! of about 185 in sRGB; untagged it would be 130.
//!
//!   cargo run --release -p app_server --example color_probe [--untagged]
//!
//! Run in a test session (`tsession run NAME …`), never on the person's.

use std::{io::Write, os::fd::AsFd, time::Duration};

use wayland_client::{
    Connection, Dispatch, QueueHandle, WEnum, delegate_noop,
    protocol::{wl_buffer, wl_compositor, wl_output, wl_registry, wl_shm, wl_shm_pool, wl_surface},
};
use wayland_protocols::{
    wp::color_management::v1::client::{
        wp_color_management_output_v1, wp_color_management_surface_v1, wp_color_manager_v1, wp_image_description_creator_params_v1, wp_image_description_info_v1, wp_image_description_v1,
    },
    xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base},
};

#[derive(Default)]
struct App {
    compositor: Option<wl_compositor::WlCompositor>,
    shm: Option<wl_shm::WlShm>,
    wm: Option<xdg_wm_base::XdgWmBase>,
    manager: Option<wp_color_manager_v1::WpColorManagerV1>,
    outputs: Vec<wl_output::WlOutput>,
    ready: Option<u32>,
    failed: bool,
    configured: bool,
    info_done: bool,
}

impl Dispatch<wl_registry::WlRegistry, ()> for App {
    fn event(app: &mut Self, registry: &wl_registry::WlRegistry, event: wl_registry::Event, _: &(), _: &Connection, qh: &QueueHandle<Self>) {
        if let wl_registry::Event::Global { name, interface, version } = event {
            match interface.as_str() {
                "wl_compositor" => app.compositor = Some(registry.bind(name, 4, qh, ())),
                "wl_shm" => app.shm = Some(registry.bind(name, 1, qh, ())),
                "xdg_wm_base" => app.wm = Some(registry.bind(name, 1, qh, ())),
                "wl_output" => app.outputs.push(registry.bind(name, 1, qh, ())),
                "wp_color_manager_v1" => {
                    println!("wp_color_manager_v1 version {version}");
                    app.manager = Some(registry.bind(name, 1, qh, ()));
                }
                _ => {}
            }
        }
    }
}

impl Dispatch<wp_color_manager_v1::WpColorManagerV1, ()> for App {
    fn event(_: &mut Self, _: &wp_color_manager_v1::WpColorManagerV1, event: wp_color_manager_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wp_color_manager_v1::Event::SupportedIntent { render_intent } => println!("  intent {render_intent:?}"),
            wp_color_manager_v1::Event::SupportedFeature { feature } => println!("  feature {feature:?}"),
            wp_color_manager_v1::Event::SupportedTfNamed { tf } => println!("  transfer {tf:?}"),
            wp_color_manager_v1::Event::SupportedPrimariesNamed { primaries } => println!("  primaries {primaries:?}"),
            wp_color_manager_v1::Event::Done => println!("  done"),
            _ => {}
        }
    }
}

impl Dispatch<wp_image_description_v1::WpImageDescriptionV1, ()> for App {
    fn event(app: &mut Self, _: &wp_image_description_v1::WpImageDescriptionV1, event: wp_image_description_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        match event {
            wp_image_description_v1::Event::Ready { identity } => app.ready = Some(identity),
            wp_image_description_v1::Event::Failed { cause, msg } => {
                println!("  description failed: {cause:?} {msg}");
                app.failed = true;
            }
            _ => {}
        }
    }
}

impl Dispatch<wp_image_description_info_v1::WpImageDescriptionInfoV1, ()> for App {
    fn event(app: &mut Self, _: &wp_image_description_info_v1::WpImageDescriptionInfoV1, event: wp_image_description_info_v1::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let wp_image_description_info_v1::Event::Done = event {
            app.info_done = true;
        }
        println!("    {event:?}");
    }
}

impl Dispatch<xdg_wm_base::XdgWmBase, ()> for App {
    fn event(_: &mut Self, wm: &xdg_wm_base::XdgWmBase, event: xdg_wm_base::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_wm_base::Event::Ping { serial } = event {
            wm.pong(serial);
        }
    }
}

impl Dispatch<xdg_surface::XdgSurface, ()> for App {
    fn event(app: &mut Self, s: &xdg_surface::XdgSurface, event: xdg_surface::Event, _: &(), _: &Connection, _: &QueueHandle<Self>) {
        if let xdg_surface::Event::Configure { serial } = event {
            s.ack_configure(serial);
            app.configured = true;
        }
    }
}

delegate_noop!(App: ignore wl_compositor::WlCompositor);
delegate_noop!(App: ignore wl_shm::WlShm);
delegate_noop!(App: ignore wl_shm_pool::WlShmPool);
delegate_noop!(App: ignore wl_buffer::WlBuffer);
delegate_noop!(App: ignore wl_surface::WlSurface);
delegate_noop!(App: ignore wl_output::WlOutput);
delegate_noop!(App: ignore xdg_toplevel::XdgToplevel);
delegate_noop!(App: ignore wp_color_management_output_v1::WpColorManagementOutputV1);
delegate_noop!(App: ignore wp_color_management_surface_v1::WpColorManagementSurfaceV1);
delegate_noop!(App: ignore wp_image_description_creator_params_v1::WpImageDescriptionCreatorParamsV1);

fn main() {
    let untagged = std::env::args().any(|a| a == "--untagged");
    let conn = Connection::connect_to_env().expect("a Wayland display");
    let mut queue = conn.new_event_queue();
    let qh = queue.handle();
    let mut app = App::default();
    conn.display().get_registry(&qh, ());
    queue.roundtrip(&mut app).unwrap();
    queue.roundtrip(&mut app).unwrap();
    let manager = app.manager.clone().expect("the compositor offers wp_color_manager_v1");

    // Each display's description.
    for (i, o) in app.outputs.clone().iter().enumerate() {
        let cm = manager.get_output(o, &qh, ());
        let d = cm.get_image_description(&qh, ());
        app.ready = None;
        queue.roundtrip(&mut app).unwrap();
        println!("display {i}: description {:?}", app.ready);
        app.info_done = false;
        d.get_information(&qh, ());
        queue.roundtrip(&mut app).unwrap();
    }

    // A window, its buffer PQ's code for 100 cd/m² in every channel.
    let (w, h) = (256i32, 256i32);
    let code = (olo_color::pq_encode(100.0 / 10_000.0) * 255.0).round() as u8;
    println!("buffer: {code} in every channel ({})", if untagged { "untagged" } else { "tagged PQ, BT.2020" });
    let file = tempfile_in_runtime();
    let mut f = std::fs::File::from(file.try_clone().unwrap());
    let pixels: Vec<u8> = (0..w * h).flat_map(|_| [code, code, code, 255]).collect();
    f.write_all(&pixels).unwrap();
    let pool = app.shm.as_ref().unwrap().create_pool(file.as_fd(), w * h * 4, &qh, ());
    let buffer = pool.create_buffer(0, w, h, w * 4, wl_shm::Format::Xrgb8888, &qh, ());
    let surface = app.compositor.as_ref().unwrap().create_surface(&qh, ());
    let xdg = app.wm.as_ref().unwrap().get_xdg_surface(&surface, &qh, ());
    let top = xdg.get_toplevel(&qh, ());
    top.set_title("color probe".into());
    top.set_app_id("olo.color-probe".into());
    if !untagged {
        let params = manager.create_parametric_creator(&qh, ());
        params.set_tf_named(wp_color_manager_v1::TransferFunction::St2084Pq);
        params.set_primaries_named(wp_color_manager_v1::Primaries::Bt2020);
        let d = params.create(&qh, ());
        app.ready = None;
        queue.roundtrip(&mut app).unwrap();
        println!("PQ BT.2020 description: {:?}", app.ready);
        let cs = manager.get_surface(&surface, &qh, ());
        cs.set_image_description(&d, wp_color_manager_v1::RenderIntent::Perceptual);
    }
    surface.commit();
    while !app.configured {
        queue.blocking_dispatch(&mut app).unwrap();
    }
    surface.attach(Some(&buffer), 0, 0);
    surface.damage_buffer(0, 0, w, h);
    surface.commit();
    queue.roundtrip(&mut app).unwrap();
    println!("shown");
    let _ = WEnum::<u32>::Value(0);
    // Shown till killed.
    loop {
        queue.flush().ok();
        std::thread::sleep(Duration::from_millis(200));
        let _ = queue.dispatch_pending(&mut app);
    }
}

/// An unlinked file in the runtime folder, for the buffer.
fn tempfile_in_runtime() -> std::os::fd::OwnedFd {
    let dir = std::env::var("XDG_RUNTIME_DIR").unwrap_or_else(|_| "/var/tmp".into());
    let path = format!("{dir}/olo-color-probe-{}", std::process::id());
    let f = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(true).open(&path).unwrap();
    let _ = std::fs::remove_file(&path);
    f.into()
}
