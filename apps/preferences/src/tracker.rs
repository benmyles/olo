//! Tracker: how a folder window shows its folder when it opens. Windows
//! already open keep theirs (View menu).

use olo_protocol::FolderView;

use crate::form::Ui;

pub fn ui(ui: &mut Ui) {
    let now = ui.s.tracker.default_view;
    ui.bbox("New Windows Open In", |ui| {
        for (view, label) in [(FolderView::Columns, "Column View"), (FolderView::List, "List View"), (FolderView::Icons, "Icon View"), (FolderView::MiniIcons, "Mini Icon View")] {
            ui.radio(label, now == view, move |s| s.tracker.default_view = view);
        }
    });
}
