//! The watcher alone, for measuring it (`tools/dev/cost PID` while it
//! waits: nothing; each change logged with how long it took) and for
//! seeing what it makes of a change by hand (a cable pulled).

fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new("info")).init();
    olo_net::watch();
    println!("pid {}: online {}, generation {}", std::process::id(), olo_net::online(), olo_net::generation());
    olo_net::on_change(|| println!("changed: online {}, generation {}", olo_net::online(), olo_net::generation()));
    loop {
        std::thread::park();
    }
}
