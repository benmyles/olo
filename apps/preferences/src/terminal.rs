//! Terminal: where New Worktree… (a repository group's menu) makes a
//! repository's worktrees. Terminal follows it live.

use olo_kit::{
    egui::Ui,
    widgets::{BBox, StringView, TextControl},
};
use olo_protocol::Settings;

pub fn ui(ui: &mut Ui, s: &mut Settings) {
    let t = &mut s.terminal;
    BBox::new("Git Worktrees").show(ui, |ui| {
        ui.add(TextControl::new(&mut t.worktrees).label("New ones go in:").divider(100.0).hint("~/worktrees").mono(true));
        let base = t.worktrees.trim().trim_end_matches('/');
        let base = if base.is_empty() { "~/worktrees" } else { base };
        let example = format!("Each repository gets a folder in there: olo’s worktree fix-tabs goes in {base}/olo/fix-tabs.");
        ui.add(StringView::new(&example).dim(true).wrap(true));
    });
}
