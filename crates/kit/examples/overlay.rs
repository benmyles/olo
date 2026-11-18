//! A column list with a frame laid over it from outside (an absolute
//! canvas in the same box), as Tracker's first drop mark was, to look at
//! in a test session at every scale (`tsession run NAME
//! target/release/examples/overlay`, then `tsession scale`): that mark
//! was seen at 1× and 2× and not at 1.25×, 1.5× or 1.75×; this shows at
//! every scale (2026-11-17). `OVERLAY_EDGE=1`: the frame at the canvas's
//! own edges, round the whole list; `OVERLAY_LATER=1`: it comes a second
//! after the window, as a drag's mark does.

use olo_kit::{
    Cx, Element, ParentElement as _, Styled as _, View, WindowOptions, canvas, div, metrics as m, px,
    widgets::{Column, ColumnListView, ListModel},
};

struct Over {
    list: ListModel,
    /// Shown (`OVERLAY_LATER`: a second after the window, as a drag's
    /// mark comes).
    shown: bool,
}

impl View for Over {
    type Message = ();

    fn message(&mut self, _: (), cx: &mut Cx<Self>) {
        self.shown = true;
        cx.invalidate();
    }

    fn body(&mut self, cx: &mut Cx<Self>) -> Element {
        let columns = [Column::new("Name", 160.0), Column::new("Size", 90.0).right()];
        let list = ColumnListView::new(cx, "rows", &self.list, columns, 30, |_: &Self, cell, g| {
            cell.text(g, if cell.column == 0 { format!("Row {}", cell.row + 1) } else { "12 KB".into() });
        });
        let edge = std::env::var_os("OVERLAY_EDGE").is_some();
        let frame = canvas(move |g, b| {
            let red = olo_kit::Hsla { h: 0.0, s: 1.0, l: 0.45, a: 1.0 };
            // (A frame two points wide: four bars; `OVERLAY_EDGE`: round
            // the whole list, at the canvas's own edges.)
            let (x, y, w, h) = if edge { (b.origin.x, b.origin.y, b.size.width, b.size.height) } else { (b.origin.x + px(40.0), b.origin.y + px(60.0), px(200.0), px(60.0)) };
            for (bx, by, bw, bh) in [(x, y, w, px(2.0)), (x, y + h - px(2.0), w, px(2.0)), (x, y, px(2.0), h), (x + w - px(2.0), y, px(2.0), h)] {
                olo_kit::style::fill_rect(g, olo_kit::Bounds { origin: olo_kit::point(bx, by), size: olo_kit::size(bw, bh) }, red);
            }
        })
        .absolute()
        .inset_0();
        let mut boxed = div().relative().size_full().child(list);
        if self.shown {
            boxed = boxed.child(frame);
        }
        div().size_full().p(px(m::WINDOW_INSET)).child(boxed)
    }
}

fn main() {
    olo_kit::run("olo.overlay-example", |_| {
        let later = std::env::var_os("OVERLAY_LATER").is_some();
        olo_kit::open_window(WindowOptions::new("Overlay", 380.0, 326.0), move |cx| {
            if later {
                cx.after(std::time::Duration::from_secs(1), ());
            }
            Over { list: ListModel::new(true), shown: !later }
        });
    });
}
