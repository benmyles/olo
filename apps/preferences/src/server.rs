//! The link to app_server: a watcher thread that keeps our copy of the
//! settings and outputs current, and a writer thread that sends changes
//! (only the newest settings matter, so a slider drag sends a handful of
//! requests, not one per frame).

use std::{
    sync::mpsc::{Sender, channel},
    time::Duration,
};

use olo_protocol::{Client, Event, EventKind, OutputConfig, Reply, Request, Settings};
use olo_ui::Proxy;

use crate::Msg;

enum Out {
    Settings(Box<Settings>),
    Output(String, OutputConfig),
    Caffeinated(bool),
    Brightness(String, f32),
    KeyboardBacklight(f32),
    StartScreenSaver,
    Identify,
    RecordShortcut(bool),
    ReloadServer,
}

/// (A clone sends through the same writer.)
#[derive(Clone)]
pub struct Server {
    tx: Sender<Out>,
    /// The keys app_server recorded for a shortcut (see `record_shortcut`).
    recorded: Recorded,
}

type Recorded = std::sync::Arc<olo_sync::Mutex<Option<Option<String>>>>;

impl Server {
    pub fn start(proxy: Proxy) -> Server {
        let recorded = Recorded::default();
        watch(proxy, recorded.clone());
        let (tx, rx) = channel::<Out>();
        std::thread::Builder::new()
            .name("prefs-writer".into())
            .spawn(move || {
                let mut client: Option<Client> = None;
                while let Ok(mut out) = rx.recv() {
                    // Collapse queued settings into the newest.
                    while let Ok(next) = rx.try_recv() {
                        if supersedes(&next, &out) {
                            out = next;
                        } else {
                            send(&mut client, out);
                            out = next;
                        }
                    }
                    send(&mut client, out);
                }
            })
            .expect("thread");
        Server { tx, recorded }
    }

    /// A link to nowhere: what it's asked to send is dropped (for the
    /// layout lint).
    #[cfg(test)]
    pub fn detached() -> Server {
        Server { tx: channel().0, recorded: Recorded::default() }
    }

    pub fn set_settings(&self, settings: Settings) {
        let _ = self.tx.send(Out::Settings(Box::new(settings)));
    }

    pub fn configure_output(&self, name: &str, config: OutputConfig) {
        let _ = self.tx.send(Out::Output(name.to_owned(), config));
    }

    pub fn set_caffeinated(&self, on: bool) {
        let _ = self.tx.send(Out::Caffeinated(on));
    }

    /// Have app_server take the next keys typed as a shortcut; they come
    /// back through `take_recorded`.
    pub fn record_shortcut(&self, on: bool) {
        let _ = self.tx.send(Out::RecordShortcut(on));
    }

    /// The keys recorded since asked ("" for none), or None if it was
    /// cancelled; nothing yet: None.
    pub fn take_recorded(&self) -> Option<Option<String>> {
        self.recorded.lock().ok()?.take()
    }

    /// A display's brightness, by connector.
    pub fn set_output_brightness(&self, name: &str, level: f32) {
        let _ = self.tx.send(Out::Brightness(name.to_owned(), level));
    }

    pub fn set_keyboard_backlight(&self, level: f32) {
        let _ = self.tx.send(Out::KeyboardBacklight(level));
    }

    pub fn identify_outputs(&self) {
        let _ = self.tx.send(Out::Identify);
    }

    /// Start app_server again (a new GPU chosen takes effect so).
    pub fn reload_server(&self) {
        let _ = self.tx.send(Out::ReloadServer);
    }

    /// After any settings already queued, so it runs what is chosen.
    pub fn start_screen_saver(&self) {
        let _ = self.tx.send(Out::StartScreenSaver);
    }
}

/// Whether `next` makes `out` pointless to send.
fn supersedes(next: &Out, out: &Out) -> bool {
    match (out, next) {
        (Out::Settings(_), Out::Settings(_)) | (Out::KeyboardBacklight(_), Out::KeyboardBacklight(_)) => true,
        (Out::Brightness(a, _), Out::Brightness(b, _)) => a == b,
        _ => false,
    }
}

fn send(client: &mut Option<Client>, out: Out) {
    let req = match out {
        Out::Settings(settings) => Request::SetSettings { settings: *settings },
        Out::Output(name, config) => Request::ConfigureOutput { name, config },
        Out::Caffeinated(on) => Request::SetCaffeinated { on, minutes: None, lid: false },
        Out::Brightness(name, level) => Request::SetOutputBrightness { name, level },
        Out::KeyboardBacklight(level) => Request::SetKeyboardBacklight { level },
        Out::StartScreenSaver => Request::StartScreenSaver,
        Out::Identify => Request::IdentifyOutputs,
        Out::RecordShortcut(on) => Request::RecordShortcut { on },
        Out::ReloadServer => Request::ReloadServer,
    };
    for _ in 0..2 {
        if client.is_none() {
            *client = Client::connect().ok();
        }
        let Some(c) = client.as_mut() else { return };
        match c.request(&req) {
            Ok(Reply::Error { message }) => {
                tracing::warn!("app_server: {message}");
                return;
            }
            Ok(_) => return,
            // The connection went away (app_server reloaded): reconnect once.
            Err(_) => *client = None,
        }
    }
}

fn watch(proxy: Proxy, recorded: Recorded) {
    std::thread::Builder::new()
        .name("prefs-watch".into())
        .spawn(move || {
            loop {
                if let Ok(mut c) = Client::connect() {
                    if let Ok(Reply::Settings { settings }) = c.request(&Request::GetSettings) {
                        proxy.send(Msg::Settings(Box::new(settings)));
                    }
                    if let Ok(Reply::Outputs { outputs }) = c.request(&Request::Outputs) {
                        proxy.send(Msg::Outputs(outputs));
                    }
                    if let Ok(Reply::Power { status }) = c.request(&Request::PowerStatus) {
                        proxy.send(Msg::Power(status));
                    }
                    if let Ok(events) = c.into_events_of(&[EventKind::Settings, EventKind::Outputs, EventKind::Power, EventKind::ShortcutRecorded]) {
                        for e in events.flatten() {
                            match e {
                                Event::SettingsChanged { settings } => proxy.send(Msg::Settings(Box::new(settings))),
                                Event::OutputsChanged { outputs } => proxy.send(Msg::Outputs(outputs)),
                                Event::PowerChanged { status } => proxy.send(Msg::Power(status)),
                                Event::ShortcutRecorded { keys } => {
                                    if let Ok(mut r) = recorded.lock() {
                                        *r = Some(keys);
                                    }
                                    proxy.send(Msg::Redraw);
                                }
                                _ => {}
                            }
                        }
                    }
                }
                std::thread::sleep(Duration::from_secs(1));
            }
        })
        .expect("thread");
}
