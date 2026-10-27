//! Tracker: how a folder window shows its folder when it opens. Windows
//! already open keep theirs (View menu).

use olo_kit::{
    egui::Ui,
    widgets::{BBox, RadioButton},
};
use olo_protocol::{FolderView, Settings};

pub fn ui(ui: &mut Ui, s: &mut Settings) {
    BBox::new("New Windows Open In").show(ui, |ui| {
        for (view, label) in [
            (FolderView::Columns, "Column View"),
            (FolderView::List, "List View"),
            (FolderView::Icons, "Icon View"),
            (FolderView::MiniIcons, "Mini Icon View"),
        ] {
            if ui.add(RadioButton::new(s.tracker.default_view == view, label)).clicked() {
                s.tracker.default_view = view;
            }
        }
    });
}
