//! The Print panel on olo-kit, alone: what's chosen is printed as it
//! closes. The same as `print_panel_gpui` shows it on GPUI: the two are
//! compared pixel for pixel (`tools/dev/kit-compare`).

fn main() {
    olo_kit::run("olo.print-panel-example", |_| {
        olo_print::panel::kit::show(olo_print::panel::kit::window_options("Print"), Some(12), |choice| {
            println!("{choice:?}");
            olo_kit::quit();
        });
    });
}
