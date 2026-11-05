//! `tabula-session`: olo-session under the name it had before the
//! rename, for a session that started on a Tabula build to upgrade to
//! (see Cargo.toml and `Session::bridge` in lib.rs).

fn main() {
    olo_crash::install();
    olo_session::run();
}
