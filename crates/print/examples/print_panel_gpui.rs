//! The Print panel on GPUI, alone (see `print_panel`).

fn main() {
    olo_ui::run("olo.print-panel-example", |cx| {
        olo_print::panel::view::show(cx, olo_print::panel::view::window_options("Print"), Some(12), |choice, cx| {
            println!("{choice:?}");
            cx.quit();
        });
    });
}
