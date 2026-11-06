//! A D-Bus message bus: busd 0.5.0 (MIT, by Zeeshan Ali Khan and the
//! busd contributors; `LICENSE-MIT`), the broker's core of it, on
//! async-io rather than tokio (zbus's `tokio` feature would have turned
//! every Olo program's zbus to tokio, Cargo unifying features), Unix
//! sockets only, and lenient about match rules as other brokers are
//! (`fdo::dbus::lenient_rule`: Chrome's). An agent screen's programs get a
//! bus of their own from it (`olo-bus`, run by its olo-session), so a GTK
//! app there finds no copy of itself on the person's bus, and the screen
//! has its own notifications and portal.

pub mod bus;
pub mod fdo;
pub mod match_rules;
pub mod name_registry;
pub mod peer;
pub mod peers;

/// Run `future` to its end on smol's executor, on its own.
pub(crate) fn spawn(future: impl std::future::Future<Output = impl Send + 'static> + Send + 'static) {
    smol::spawn(future).detach();
}
