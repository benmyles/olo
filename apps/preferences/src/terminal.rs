//! Terminal: where New Worktree… (a repository group's menu) makes a
//! repository's worktrees, whether coding agents are told about the
//! repositories in their tab's group, and whether they keep their tab's
//! title current. Terminal follows them live.

use olo_ui::{gpui::Font, metrics as m};

use crate::form::Ui;

pub fn ui(ui: &mut Ui) {
    let t = ui.s.terminal.clone();
    ui.bbox("Git Worktrees", |ui| {
        let mono: Font = olo_ui::style::mono_font(olo_ui::gpui::FontWeight::NORMAL);
        ui.text("worktrees", &t.worktrees, |f| f.hint("~/worktrees").font(mono), |c| c.label("New ones go in:").divider(100.0), |s, v| s.terminal.worktrees = v);
        let base = t.worktrees.trim().trim_end_matches('/');
        let base = if base.is_empty() { "~/worktrees" } else { base };
        ui.note(format!("Each repository gets a folder in there: olo’s worktree fix-tabs goes in {base}/olo/fix-tabs."));
    });
    ui.bbox("Coding Agents", |ui| {
        ui.check("Tell coding agents about the repositories in their tab group", t.agent_context, |s, v| s.terminal.agent_context = v);
        ui.note(
            "Claude Code and Codex started in a tab of a group whose tabs are in more than one repository are told the group’s name and those repositories and worktrees, theirs marked. Codex asks you once to trust Terminal’s hook (/hooks).",
        );
        ui.space(m::TIGHT);
        ui.check("Let coding agents keep their tab’s title current", t.agent_titles, |s, v| s.terminal.agent_titles = v);
        ui.note("Claude Code and Codex get a tool to title their tab with what they’re working on, and are asked to keep it current. They see the tool from their next start.");
    });
}
