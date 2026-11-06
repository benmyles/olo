//! olo-bus: an agent screen's own session bus (`olo-bus ADDRESS [--a11y
//! ADDRESS] [--ready]`), which its olo-session runs before anything else
//! (`--own-bus`) and ends with it.
//!
//! On it, `org.a11y.Bus` gives the accessibility bus the person's
//! programs use (`--a11y`), so GTK and Qt programs on the screen show
//! their trees there as they did on the person's bus (`olo-screen tree`
//! reads them by pid); without it they'd keep their accessibility to
//! themselves. `--ready` prints "ready" once it listens.

use std::{
    ffi::CString,
    os::unix::ffi::{OsStrExt as _, OsStringExt as _},
    path::PathBuf,
};

use olo_bus::bus::Bus;

/// The socket file, removed as the bus ends (SIGTERM).
static mut SOCKET: Option<CString> = None;

extern "C" fn end(_: libc::c_int) {
    // SAFETY: set once before the handler is installed; unlink and _exit
    // are async-signal-safe.
    unsafe {
        if let Some(path) = &*std::ptr::addr_of!(SOCKET) {
            libc::unlink(path.as_ptr());
        }
        libc::_exit(0);
    }
}

/// `org.a11y.Bus` and `org.a11y.Status`, as at-spi-bus-launcher serves
/// them, for the person's accessibility bus.
struct A11y {
    address: String,
}

#[zbus::interface(name = "org.a11y.Bus")]
impl A11y {
    fn get_address(&self) -> String {
        self.address.clone()
    }
}

struct A11yStatus;

#[zbus::interface(name = "org.a11y.Status")]
impl A11yStatus {
    #[zbus(property)]
    fn is_enabled(&self) -> bool {
        true
    }

    #[zbus(property)]
    fn set_is_enabled(&mut self, _value: bool) {}

    #[zbus(property)]
    fn screen_reader_enabled(&self) -> bool {
        false
    }

    #[zbus(property)]
    fn set_screen_reader_enabled(&mut self, _value: bool) {}
}

/// What xdg-desktop-portal keeps of what was allowed (devices, Open
/// With's choices, screenshots), as xdg-permission-store serves it, but
/// in memory: a screen's choices end with it, and the person's own store
/// (on disk, theirs) is never written by two at once. The portal won't
/// start without one, and this bus starts nothing on demand.
#[derive(Default)]
struct PermissionStore {
    /// (table, id) → each app's permissions, and the entry's data.
    entries: std::collections::BTreeMap<(String, String), (Permissions, zbus::zvariant::OwnedValue)>,
}

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.portal.Error")]
enum StoreError {
    NotFound(String),
}

type Permissions = std::collections::HashMap<String, Vec<String>>;

fn empty() -> zbus::zvariant::OwnedValue {
    zbus::zvariant::OwnedValue::from(0u8)
}

#[zbus::interface(name = "org.freedesktop.impl.portal.PermissionStore")]
impl PermissionStore {
    #[zbus(property)]
    fn version(&self) -> u32 {
        2
    }

    fn lookup(&self, table: String, id: String) -> Result<(Permissions, zbus::zvariant::OwnedValue), StoreError> {
        let (perms, data) = self.entries.get(&(table, id.clone())).ok_or(StoreError::NotFound(format!("no entry for {id}")))?;
        Ok((perms.clone(), data.try_clone().unwrap_or_else(|_| empty())))
    }

    fn get_permission(&self, table: String, id: String, app: String) -> Vec<String> {
        self.entries.get(&(table, id)).and_then(|(p, _)| p.get(&app).cloned()).unwrap_or_default()
    }

    fn list(&self, table: String) -> Vec<String> {
        self.entries.keys().filter(|(t, _)| *t == table).map(|(_, id)| id.clone()).collect()
    }

    async fn set(
        &mut self,
        table: String,
        _create: bool,
        id: String,
        app_permissions: Permissions,
        data: zbus::zvariant::OwnedValue,
        #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>,
    ) {
        let data2 = data.try_clone().unwrap_or_else(|_| empty());
        self.entries.insert((table.clone(), id.clone()), (app_permissions.clone(), data));
        let _ = Self::changed(&emitter, &table, &id, false, &data2.into(), app_permissions).await;
    }

    async fn set_value(&mut self, table: String, _create: bool, id: String, data: zbus::zvariant::OwnedValue, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>) {
        let entry = self.entries.entry((table.clone(), id.clone())).or_insert_with(|| (Permissions::new(), empty()));
        entry.1 = data;
        let (perms, data) = (entry.0.clone(), entry.1.try_clone().unwrap_or_else(|_| empty()));
        let _ = Self::changed(&emitter, &table, &id, false, &data.into(), perms).await;
    }

    async fn set_permission(&mut self, table: String, _create: bool, id: String, app: String, permissions: Vec<String>, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>) {
        let entry = self.entries.entry((table.clone(), id.clone())).or_insert_with(|| (Permissions::new(), empty()));
        entry.0.insert(app, permissions);
        let (perms, data) = (entry.0.clone(), entry.1.try_clone().unwrap_or_else(|_| empty()));
        let _ = Self::changed(&emitter, &table, &id, false, &data.into(), perms).await;
    }

    async fn delete(&mut self, table: String, id: String, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>) -> Result<(), StoreError> {
        let (perms, data) = self.entries.remove(&(table.clone(), id.clone())).ok_or(StoreError::NotFound(format!("no entry for {id}")))?;
        let _ = Self::changed(&emitter, &table, &id, true, &data.into(), perms).await;
        Ok(())
    }

    async fn delete_permission(&mut self, table: String, id: String, app: String, #[zbus(signal_emitter)] emitter: zbus::object_server::SignalEmitter<'_>) -> Result<(), StoreError> {
        let entry = self.entries.get_mut(&(table.clone(), id.clone())).ok_or(StoreError::NotFound(format!("no entry for {id}")))?;
        entry.0.remove(&app);
        let (perms, data) = (entry.0.clone(), entry.1.try_clone().unwrap_or_else(|_| empty()));
        let _ = Self::changed(&emitter, &table, &id, false, &data.into(), perms).await;
        Ok(())
    }

    #[zbus(signal)]
    async fn changed(emitter: &zbus::object_server::SignalEmitter<'_>, table: &str, id: &str, deleted: bool, data: &zbus::zvariant::Value<'_>, permissions: Permissions) -> zbus::Result<()>;
}

/// The document portal, as far as xdg-desktop-portal asks it when it
/// starts (where its files are: the person's, which their own document
/// portal serves). Sandboxed programs' file sharing isn't offered on a
/// screen; a second document portal couldn't mount over the person's.
struct Documents;

#[zbus::interface(name = "org.freedesktop.portal.Documents")]
impl Documents {
    #[zbus(property)]
    fn version(&self) -> u32 {
        4
    }

    fn get_mount_point(&self) -> Vec<u8> {
        let rt = std::env::var_os("XDG_RUNTIME_DIR").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("/run/user").join(rustix_uid()));
        let mut path = rt.join("doc").into_os_string().into_vec();
        path.push(0);
        path
    }
}

fn rustix_uid() -> String {
    // SAFETY: getuid can't fail.
    unsafe { libc::getuid() }.to_string()
}

fn main() -> std::process::ExitCode {
    olo_crash::install();
    let filter = tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(address) = args.first().filter(|a| !a.starts_with("--")).cloned() else {
        eprintln!("usage: olo-bus unix:path=SOCKET [--a11y ADDRESS] [--expect NAME]… [--ready] [--with-parent]");
        return std::process::ExitCode::from(2);
    };
    let a11y = args.iter().position(|a| a == "--a11y").and_then(|i| args.get(i + 1)).cloned();
    let ready = args.iter().any(|a| a == "--ready");
    olo_bus::fdo::expect(args.windows(2).filter(|w| w[0] == "--expect").map(|w| w[1].clone()).collect());
    if let Some(path) = address.strip_prefix("unix:path=").map(|p| p.split(',').next().unwrap_or(p)) {
        // A socket left by a bus that died: ours now.
        let _ = std::fs::remove_file(path);
        // SAFETY: before the handler and any thread.
        unsafe { *std::ptr::addr_of_mut!(SOCKET) = CString::new(PathBuf::from(path).as_os_str().as_bytes()).ok() };
    }
    // SAFETY: the handler only unlinks and exits; with `--with-parent`,
    // ending with our parent (the screen's olo-session) however it ends.
    unsafe {
        // (olo-session blocks its signals to read them itself: a mask
        // inherited from it would keep SIGTERM from ever coming.)
        let mut none: libc::sigset_t = std::mem::zeroed();
        libc::sigemptyset(&mut none);
        libc::sigprocmask(libc::SIG_SETMASK, &none, std::ptr::null_mut());
        libc::signal(libc::SIGTERM, end as *const () as libc::sighandler_t);
        libc::signal(libc::SIGINT, end as *const () as libc::sighandler_t);
        if args.iter().any(|a| a == "--with-parent") {
            libc::prctl(libc::PR_SET_PDEATHSIG, libc::SIGTERM as libc::c_ulong, 0, 0, 0);
        }
    }
    let result: anyhow::Result<()> = smol::block_on(async {
        let mut bus = Bus::for_address(Some(&address)).await?;
        // Its services, served by a client of the bus as any service is,
        // there before it says it's ready.
        let services = {
            let address = address.clone();
            async move {
                let mut b = zbus::connection::Builder::address(address.as_str())?
                    .name("org.freedesktop.impl.portal.PermissionStore")?
                    .serve_at("/org/freedesktop/impl/portal/PermissionStore", PermissionStore::default())?
                    .name("org.freedesktop.portal.Documents")?
                    .serve_at("/org/freedesktop/portal/documents", Documents)?;
                if let Some(a11y) = a11y {
                    b = b.name("org.a11y.Bus")?.serve_at("/org/a11y/bus", A11y { address: a11y })?.serve_at("/org/a11y/bus", A11yStatus)?;
                }
                anyhow::Ok(b.build().await?)
            }
        };
        // (The bus answers while its services connect.)
        let running = smol::spawn(async move { bus.run().await });
        let conn = services.await?;
        if ready {
            println!("ready");
        }
        let r = running.await;
        drop(conn);
        r
    });
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("olo-bus: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}
