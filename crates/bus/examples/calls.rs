//! How fast calls go through a bus: `cargo run --release -p olo-bus
//! --example calls -- unix:path=SOCKET [N]` against a running `olo-bus`
//! times N calls to the bus itself (`GetId`) and N to another peer (an
//! `Echo` served here), each answered before the next is made.

struct Echo;

#[zbus::interface(name = "org.olo.Test.Echo")]
impl Echo {
    fn echo(&self, s: String) -> String {
        s
    }
}

fn main() {
    let mut args = std::env::args().skip(1);
    let address = args.next().expect("usage: calls ADDRESS [N]");
    let n: u32 = args.next().map_or(20_000, |n| n.parse().expect("N"));
    let _service = zbus::blocking::connection::Builder::address(address.as_str()).unwrap().name("org.olo.Test.Echo").unwrap().serve_at("/", Echo).unwrap().build().unwrap();
    let caller = zbus::blocking::connection::Builder::address(address.as_str()).unwrap().build().unwrap();
    let dbus = zbus::blocking::fdo::DBusProxy::new(&caller).unwrap();
    let t = std::time::Instant::now();
    for _ in 0..n {
        dbus.get_id().unwrap();
    }
    let bus = t.elapsed();
    let t = std::time::Instant::now();
    for _ in 0..n {
        let _: String = caller.call_method(Some("org.olo.Test.Echo"), "/", Some("org.olo.Test.Echo"), "Echo", &("hi",)).unwrap().body().deserialize().unwrap();
    }
    let peer = t.elapsed();
    println!("{n} calls: to the bus {:.1} µs each, to a peer {:.1} µs each", bus.as_secs_f64() * 1e6 / n as f64, peer.as_secs_f64() * 1e6 / n as f64);
}
