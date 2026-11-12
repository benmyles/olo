//! The Print panel, alone: what's chosen is printed as it closes (to
//! compare with a build from before a change: `tools/dev/kit-compare`).

fn main() {
    olo_kit::run("olo.print-panel-example", |_| {
        olo_print::panel::kit::show(olo_print::panel::kit::window_options("Print"), Some(12), |choice| {
            println!("{choice:?}");
            olo_kit::quit();
        });
    });
}
