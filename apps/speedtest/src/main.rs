//! Network Speed Test: what the connection carries each way, and
//! everything that can be measured or read of it as it does (Ben,
//! 2026-11-16: "add a new olo native Network Speed Test app that show not
//! just total up/down but also all the advaned metrics/data about the
//! connection that we are able to get").
//!
//! - engine.rs: the test (against Cloudflare's speed test endpoints):
//!   the round trip idle and loaded, the download and the upload over
//!   several connections, the kernel's own figures for each socket
//!   (`olo_net::tcp_info`: the congestion control and its window, pacing
//!   and delivery rates, what was sent again, BBR's model) and the
//!   server's for its side; http.rs: its connections.
//! - view.rs: the window: four cards (Download, Upload, Latency,
//!   Connection), each a figure, a graph and its facts, live as the test
//!   runs; words.rs: how the figures are said.
//!
//! `olo-speedtest --print` runs a test with no window and prints its
//! results as JSON, for agents and scripts (half a minute; it moves as
//! much data as the line carries in sixteen seconds). `olo-speedtest
//! --serve PORT [BYTES-A-SECOND]` is a speed test server of Olo's own on
//! this computer (serve.rs), which `OLO_SPEEDTEST_SERVER=127.0.0.1:PORT`
//! has the test run against: for trying it without the internet.

mod demo;
mod engine;
mod http;
#[cfg(test)]
mod lint;
mod serve;
mod view;
mod words;

use olo_kit::instance::{self, Launch};

const APP_ID: &str = "olo.speedtest";

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--print") {
        let stop = std::sync::atomic::AtomicBool::new(false);
        return match engine::run(&engine::Config::default(), &stop, &|_| {}) {
            Ok(mut results) => {
                let link = view::Link::now();
                // (The whole of it; the series are what a graph is drawn
                // from.)
                let mut json = serde_json::to_value(&results).unwrap_or_default();
                json["link"] = serde_json::to_value(&link).unwrap_or_default();
                json["added_latency_ms"] = serde_json::to_value(results.added_latency_ms()).unwrap_or_default();
                json["responsiveness_rpm"] = serde_json::to_value(results.responsiveness()).unwrap_or_default();
                results.stopped = false;
                println!("{}", serde_json::to_string_pretty(&json).unwrap_or_default());
                std::process::ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("olo-speedtest: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if args.first().is_some_and(|a| a == "--serve") {
        let port = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(0);
        let rate = args.get(2).and_then(|r| r.parse().ok()).unwrap_or(0);
        return match serve::start_at(port, rate) {
            Ok(server) => {
                println!("olo-speedtest: serving on {}", server.addr);
                loop {
                    std::thread::park();
                }
            }
            Err(e) => {
                eprintln!("olo-speedtest: port {port}: {e}");
                std::process::ExitCode::FAILURE
            }
        };
    }
    if instance::forward(APP_ID, &args) {
        return std::process::ExitCode::SUCCESS;
    }
    olo_kit::run(APP_ID, |app| {
        let window = olo_kit::open_window(view::options(), view::Speed::new);
        instance::listen(APP_ID, olo_kit::proxy());
        olo_kit::message_handler(app, move |message, _| {
            if message.is::<Launch>() {
                olo_kit::raise(Some(&window));
            }
        });
    });
    std::process::ExitCode::SUCCESS
}
