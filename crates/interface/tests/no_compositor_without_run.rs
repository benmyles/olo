//! A program that runs no app never connects to a compositor: a unit
//! test that measures text gets the display without one, whatever the
//! environment it was started from names (an agent's shell has the
//! person's `WAYLAND_DISPLAY`). Its own test program, for the
//! environment it sets.

#[test]
fn a_display_asked_for_without_run_has_no_compositor() {
    // SAFETY: this program's only test, before it starts any thread.
    unsafe {
        std::env::set_var("WAYLAND_DISPLAY", "wayland-of-nobody");
        std::env::remove_var("OLO_HEADLESS");
    }
    // (What a widget's test does: asks for the text system, which
    // starts the display thread.)
    let _ = olo_interface::text_system();
    assert!(!olo_interface::has_compositor(), "a program that runs no app connected to WAYLAND_DISPLAY");
}
