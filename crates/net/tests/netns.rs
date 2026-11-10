//! The watcher against real changes: in a network of the test's own (a
//! user and network namespace, no privileges needed), addresses and routes
//! are added and taken away as a cable pulled or Wi-Fi left would, and a
//! connection waiting on an address that went is cut at once, one whose
//! way out changed is watched closely, and a wait for the network ends
//! when a way out comes. (Where namespaces can't be made, it says so and
//! passes.)

use std::{
    io::Read,
    net::{Ipv4Addr, SocketAddr, TcpListener, TcpStream},
    os::{fd::AsRawFd, unix::process::CommandExt},
    process::Command,
    time::{Duration, Instant},
};

const INSIDE: &str = "OLO_NET_TEST_INSIDE";

/// The test, run again in a namespace of its own.
#[test]
fn network_changes() {
    if std::env::var_os(INSIDE).is_some() {
        inside();
        return;
    }
    // (Made before forking: the child only makes system calls.)
    // SAFETY: plain getters.
    let (uid, gid) = unsafe { (libc::getuid(), libc::getgid()) };
    let uid_map = format!("0 {uid} 1");
    let gid_map = format!("0 {gid} 1");
    let mut cmd = Command::new(std::env::current_exe().unwrap());
    cmd.args(["--exact", "network_changes", "--nocapture", "--test-threads", "1"]).env(INSIDE, "1");
    // SAFETY: only system calls on memory made before the fork.
    unsafe {
        cmd.pre_exec(move || {
            if libc::unshare(libc::CLONE_NEWUSER | libc::CLONE_NEWNET) != 0 {
                return Err(std::io::Error::last_os_error());
            }
            for (path, text) in [(c"/proc/self/setgroups", "deny"), (c"/proc/self/uid_map", uid_map.as_str()), (c"/proc/self/gid_map", gid_map.as_str())] {
                let fd = libc::open(path.as_ptr(), libc::O_WRONLY);
                if fd < 0 || libc::write(fd, text.as_ptr().cast(), text.len()) < 0 {
                    return Err(std::io::Error::last_os_error());
                }
                libc::close(fd);
            }
            Ok(())
        });
    }
    match cmd.output() {
        Ok(out) => {
            let said = format!("{}{}", String::from_utf8_lossy(&out.stdout), String::from_utf8_lossy(&out.stderr));
            assert!(out.status.success(), "inside the namespace:\n{said}");
            assert!(said.contains("1 passed"), "{said}");
            for line in said.lines().filter(|l| l.contains("cut ") || l.contains("the wait") || l.contains("network: changed")) {
                println!("{line}");
            }
        }
        Err(e) => eprintln!("no network namespace of its own here ({e}): not tested"),
    }
}

fn inside() {
    // (The watcher's own account of each change, with how long it took.)
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::new("info")).with_ansi(false).init();
    let nl = Rtnl::open();
    // Only loopback, down, to begin with.
    nl.link_up(1);
    nl.address(true, 1, [10, 9, 0, 1]);
    nl.address(true, 1, [10, 9, 0, 2]);
    nl.route(true, None, 1, None);
    olo_net::watch();
    assert!(olo_net::online(), "a default route: online");
    let g0 = olo_net::generation();

    // A connection from 10.9.0.1, waiting to read: its address goes, and
    // it's cut at once.
    let l = TcpListener::bind("10.9.0.2:0").unwrap();
    let mut c = connect_from([10, 9, 0, 1], l.local_addr().unwrap());
    let (_served, _) = l.accept().unwrap();
    let reader = std::thread::spawn(move || {
        let mut b = [0u8; 1];
        let r = c.read(&mut b).map_err(|e| e.kind());
        (r, Instant::now())
    });
    std::thread::sleep(Duration::from_millis(200));
    let pulled = Instant::now();
    nl.address(false, 1, [10, 9, 0, 1]);
    let (r, woke) = reader.join().unwrap();
    let took = woke - pulled;
    println!("cut {took:?} after its address went: {r:?}");
    assert!(took < Duration::from_secs(2), "{took:?}");
    assert!(olo_net::generation() > g0);

    // One from 10.9.0.2 to 10.9.0.3, and then the way to 10.9.0.3 is
    // from 10.9.0.4: watched closely (the kernel gives up on it if it
    // stops answering), not cut.
    nl.address(true, 1, [10, 9, 0, 3]);
    let l3 = TcpListener::bind("10.9.0.3:0").unwrap();
    let c2 = connect_from([10, 9, 0, 2], l3.local_addr().unwrap());
    let (_served2, _) = l3.accept().unwrap();
    assert_eq!(user_timeout(&c2), 0);
    let g1 = olo_net::generation();
    nl.address(true, 1, [10, 9, 0, 4]);
    nl.route(true, Some([10, 9, 0, 3]), 1, Some([10, 9, 0, 4]));
    wait_for(|| olo_net::generation() > g1);
    assert_eq!(user_timeout(&c2), 15_000, "watched closely");
    let mut probe = [0u8; 1];
    c2.set_nonblocking(true).unwrap();
    assert_eq!((&c2).read(&mut probe).map_err(|e| e.kind()), Err(std::io::ErrorKind::WouldBlock), "still open");

    // No way out: offline; a wait for the network ends when one comes.
    nl.route(false, None, 1, None);
    wait_for(|| !olo_net::online());
    let waiter = std::thread::spawn(|| {
        let ok = olo_net::wait_online(Some(Instant::now() + Duration::from_secs(10)), &|| false);
        (ok, Instant::now())
    });
    std::thread::sleep(Duration::from_millis(300));
    assert_eq!(olo_net::waiting(), 1);
    let back = Instant::now();
    nl.route(true, None, 1, None);
    let (ok, woke) = waiter.join().unwrap();
    println!("the wait ended {:?} after a way out came", woke - back);
    assert!(ok && woke - back < Duration::from_secs(2));
    assert_eq!(olo_net::waiting(), 0);
}

fn wait_for(f: impl Fn() -> bool) {
    let t = Instant::now();
    while !f() {
        assert!(t.elapsed() < Duration::from_secs(3), "not in 3 s");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn connect_from(from: [u8; 4], to: SocketAddr) -> TcpStream {
    // SAFETY: a socket, bound and connected, then owned by the stream.
    unsafe {
        let fd = libc::socket(libc::AF_INET, libc::SOCK_STREAM | libc::SOCK_CLOEXEC, 0);
        assert!(fd >= 0);
        let addr =
            |ip: Ipv4Addr, port: u16| libc::sockaddr_in { sin_family: libc::AF_INET as u16, sin_port: port.to_be(), sin_addr: libc::in_addr { s_addr: u32::from(ip).to_be() }, sin_zero: [0; 8] };
        let me = addr(Ipv4Addr::from(from), 0);
        assert_eq!(libc::bind(fd, (&me as *const libc::sockaddr_in).cast(), std::mem::size_of::<libc::sockaddr_in>() as u32), 0);
        let SocketAddr::V4(to) = to else { panic!() };
        let them = addr(*to.ip(), to.port());
        assert_eq!(libc::connect(fd, (&them as *const libc::sockaddr_in).cast(), std::mem::size_of::<libc::sockaddr_in>() as u32), 0, "{}", std::io::Error::last_os_error());
        std::os::fd::FromRawFd::from_raw_fd(fd)
    }
}

fn user_timeout(s: &TcpStream) -> i32 {
    let mut v: libc::c_int = 0;
    let mut len = 4;
    // SAFETY: an int option into our int.
    unsafe { libc::getsockopt(s.as_raw_fd(), libc::IPPROTO_TCP, libc::TCP_USER_TIMEOUT, (&mut v as *mut libc::c_int).cast(), &mut len) };
    v
}

/// rtnetlink for changing the namespace's network (as `ip` would).
struct Rtnl(i32);

impl Rtnl {
    fn open() -> Rtnl {
        // SAFETY: plain socket(2).
        let fd = unsafe { libc::socket(libc::AF_NETLINK, libc::SOCK_RAW | libc::SOCK_CLOEXEC, libc::NETLINK_ROUTE) };
        assert!(fd >= 0);
        Rtnl(fd)
    }

    /// Send `kind` with `body`, and check the kernel's answer.
    fn ask(&self, kind: u16, flags: u16, body: &[u8]) {
        let mut msg = Vec::new();
        msg.extend_from_slice(&((16 + body.len()) as u32).to_ne_bytes());
        msg.extend_from_slice(&kind.to_ne_bytes());
        msg.extend_from_slice(&(libc::NLM_F_REQUEST as u16 | libc::NLM_F_ACK as u16 | flags).to_ne_bytes());
        msg.extend_from_slice(&1u32.to_ne_bytes());
        msg.extend_from_slice(&0u32.to_ne_bytes());
        msg.extend_from_slice(body);
        // SAFETY: a zeroed sockaddr_nl is the kernel's; buffers ours.
        unsafe {
            let mut to: libc::sockaddr_nl = std::mem::zeroed();
            to.nl_family = libc::AF_NETLINK as u16;
            assert!(libc::sendto(self.0, msg.as_ptr().cast(), msg.len(), 0, (&to as *const libc::sockaddr_nl).cast(), std::mem::size_of::<libc::sockaddr_nl>() as u32) > 0);
            let mut buf = [0u8; 4096];
            let n = libc::recv(self.0, buf.as_mut_ptr().cast(), buf.len(), 0);
            assert!(n >= 20);
            let errno = i32::from_ne_bytes(buf[16..20].try_into().unwrap());
            assert_eq!(errno, 0, "netlink {kind}: {}", std::io::Error::from_raw_os_error(-errno));
        }
    }

    fn link_up(&self, index: i32) {
        let mut b = vec![0u8; 16];
        b[4..8].copy_from_slice(&index.to_ne_bytes());
        b[8..12].copy_from_slice(&(libc::IFF_UP as u32).to_ne_bytes());
        b[12..16].copy_from_slice(&(libc::IFF_UP as u32).to_ne_bytes());
        self.ask(libc::RTM_NEWLINK, 0, &b);
    }

    fn address(&self, add: bool, index: u32, ip: [u8; 4]) {
        let mut b = vec![libc::AF_INET as u8, 32, 0, 0];
        b.extend_from_slice(&index.to_ne_bytes());
        attr(&mut b, libc::IFA_LOCAL, &ip);
        attr(&mut b, libc::IFA_ADDRESS, &ip);
        if add {
            self.ask(libc::RTM_NEWADDR, (libc::NLM_F_CREATE | libc::NLM_F_EXCL) as u16, &b);
        } else {
            self.ask(libc::RTM_DELADDR, 0, &b);
        }
    }

    /// A route to `to` (None: the default) through link `index`, from
    /// `from` if given.
    fn route(&self, add: bool, to: Option<[u8; 4]>, index: u32, from: Option<[u8; 4]>) {
        let mut b = vec![libc::AF_INET as u8, if to.is_some() { 32 } else { 0 }, 0, 0, libc::RT_TABLE_MAIN, libc::RTPROT_BOOT, libc::RT_SCOPE_LINK, libc::RTN_UNICAST];
        b.extend_from_slice(&0u32.to_ne_bytes());
        if let Some(to) = to {
            attr(&mut b, libc::RTA_DST, &to);
        }
        attr(&mut b, libc::RTA_OIF, &index.to_ne_bytes());
        if let Some(from) = from {
            attr(&mut b, libc::RTA_PREFSRC, &from);
        }
        if add {
            self.ask(libc::RTM_NEWROUTE, (libc::NLM_F_CREATE | libc::NLM_F_EXCL) as u16, &b);
        } else {
            self.ask(libc::RTM_DELROUTE, 0, &b);
        }
    }
}

fn attr(out: &mut Vec<u8>, kind: u16, data: &[u8]) {
    out.extend_from_slice(&((4 + data.len()) as u16).to_ne_bytes());
    out.extend_from_slice(&kind.to_ne_bytes());
    out.extend_from_slice(data);
    out.resize((out.len() + 3) & !3, 0);
}
