//! Load libinput plugins from a directory and print what a device does
//! through them: `touch_filter_probe PLUGIN_DIR /dev/input/eventN SECONDS`
//! (root, read-only: the device is not grabbed). Checks Olo's touch filter
//! (touchpad.lua) against a real pad.

use std::os::fd::{AsFd, OwnedFd};
use std::os::unix::fs::OpenOptionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

use input::event::{Event, PointerEvent};
use input::{Libinput, LibinputInterface};

struct Open;

impl LibinputInterface for Open {
    fn open_restricted(&mut self, path: &Path, flags: i32) -> Result<OwnedFd, i32> {
        std::fs::OpenOptions::new().read(true).custom_flags(flags & !libc::O_ACCMODE).open(path).map(Into::into).map_err(|e| e.raw_os_error().unwrap_or(-1))
    }
    fn close_restricted(&mut self, fd: OwnedFd) {
        drop(fd);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (dir, dev, secs) = (&args[1], &args[2], args.get(3).and_then(|s| s.parse().ok()).unwrap_or(10u64));
    let mut li = Libinput::new_from_path(Open);
    li.plugin_system_append_path(dir);
    println!("load_plugins: {:?}", li.plugin_system_load_plugins(input::PluginSystemFlags::empty()));
    println!("device: {:?}", li.path_add_device(dev).map(|d| d.name().into_owned()));
    let end = Instant::now() + Duration::from_secs(secs);
    let (mut motions, mut scrolls) = (0, 0);
    while Instant::now() < end {
        let mut fds = [libc::pollfd { fd: std::os::fd::AsRawFd::as_raw_fd(&li.as_fd()), events: libc::POLLIN, revents: 0 }];
        unsafe { libc::poll(fds.as_mut_ptr(), 1, 100) };
        li.dispatch().unwrap();
        for e in &mut li {
            match e {
                Event::Pointer(PointerEvent::Motion(_)) => motions += 1,
                Event::Pointer(PointerEvent::ScrollFinger(_)) => scrolls += 1,
                Event::Pointer(PointerEvent::Button(b)) => println!("{:.3} button {} {:?}", Instant::now().elapsed().as_secs_f32(), b.button(), b.button_state()),
                other => println!("{other:?}"),
            }
        }
    }
    println!("motions {motions}, scroll frames {scrolls}");
}
