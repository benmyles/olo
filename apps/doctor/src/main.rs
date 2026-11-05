//! Olo Doctor: looks the computer over for what gets in Olo's way and
//! fixes it with a button (the window and command line: app.rs).

fn main() {
    olo_crash::install();
    olo_doctor::run(olo_doctor::Kind::Doctor);
}
