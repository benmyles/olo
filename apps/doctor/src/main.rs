//! Olo Doctor: looks the computer over for what gets in Olo's way and
//! what Olo suggests, and fixes or applies each with a button (the
//! window and command line: app.rs).

fn main() {
    olo_crash::install();
    olo_doctor::run();
}
