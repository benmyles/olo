//! `olo-rex-link agent …`: the host's agents service
//! ([`service`], `agent serve`) and the command that asks it
//! ([`client`]: `agent start|list|status|read|watch|send|approve|stop|
//! prune|focus`, what `olo agent` runs; `agent follow`, `agent request`
//! and `agent adopt`, the Olo desktop's), with the Rex server spoken to
//! as the service speaks to it ([`rex`]). The children, what is asked
//! and how it is said are the library's (`olo_rex_link::agents`).

mod client;
mod leave;
mod ops;
pub mod rex;
pub mod service;

pub fn run(args: Vec<String>) -> ! {
    std::process::exit(client::run(args))
}
