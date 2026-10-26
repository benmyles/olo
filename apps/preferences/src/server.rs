//! The link to app_server: a watcher thread that keeps our copy of the
//! settings and outputs current, and a writer thread that sends changes
//! (only the newest settings matter, so a slider drag sends a handful of
//! requests, not one per frame).

use std::{
    sync::mpsc::{Sender, channel},
    time::Duration,
};

use tabula_kit::Proxy;
use tabula_protocol::{Client, Event, OutputConfig, Reply, Request, Settings};

use crate::Msg;

enum Out {
    Settings(Box<Settings>),
    Output(String, OutputConfig),
}

pub struct Server {
    tx: Sender<Out>,
}

impl Server {
    pub fn start(proxy: Proxy) -> Server {
        watch(proxy);
        let (tx, rx) = channel::<Out>();
        std::thread::Builder::new()
            .name("prefs-writer".into())
            .spawn(move || {
                let mut client: Option<Client> = None;
                while let Ok(mut out) = rx.recv() {
                    // Collapse queued settings into the newest.
                    while let Ok(next) = rx.try_recv() {
                        if let (Out::Settings(_), Out::Settings(_)) = (&out, &next) {
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
        Server { tx }
    }

    pub fn set_settings(&self, settings: Settings) {
        let _ = self.tx.send(Out::Settings(Box::new(settings)));
    }

    pub fn configure_output(&self, name: &str, config: OutputConfig) {
        let _ = self.tx.send(Out::Output(name.to_owned(), config));
    }
}

fn send(client: &mut Option<Client>, out: Out) {
    let req = match out {
        Out::Settings(settings) => Request::SetSettings { settings: *settings },
        Out::Output(name, config) => Request::ConfigureOutput { name, config },
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

fn watch(proxy: Proxy) {
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
                    if let Ok(events) = c.into_events() {
                        for e in events.flatten() {
                            match e {
                                Event::SettingsChanged { settings } => proxy.send(Msg::Settings(Box::new(settings))),
                                Event::OutputsChanged { outputs } => proxy.send(Msg::Outputs(outputs)),
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
