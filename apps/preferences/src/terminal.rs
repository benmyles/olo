//! Terminal: where New Worktree… (a repository group's menu) makes a
//! repository's worktrees, whether coding agents are told about the
//! repositories in their tab's group, and whether they keep their tab's
//! title current. Terminal follows them live.

use olo_kit::{
    egui::Ui,
    metrics as m,
    widgets::{BBox, CheckBox, StringView, TextControl},
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
    BBox::new("Coding Agents").show(ui, |ui| {
        ui.add(CheckBox::new(&mut t.agent_context, "Tell coding agents about the repositories in their tab group"));
        ui.add(
            StringView::new(
                "Claude Code and Codex started in a tab of a group whose tabs are in more than one repository are told the group’s name and those repositories and worktrees, theirs marked. Codex asks you once to trust Terminal’s hook (/hooks).",
            )
            .dim(true)
            .wrap(true),
        );
        ui.add_space(m::TIGHT);
        ui.add(CheckBox::new(&mut t.agent_titles, "Let coding agents keep their tab’s title current"));
        ui.add(
            StringView::new(
                "Claude Code and Codex get a tool to title their tab with what they’re working on, and are asked to keep it current. They see the tool from their next start.",
            )
            .dim(true)
            .wrap(true),
        );
    });
}
