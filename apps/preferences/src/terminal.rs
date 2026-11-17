//! Terminal: where New Worktree… (a repository group's menu) makes a
//! repository's worktrees, whether coding agents are told about the
//! repositories in their tab's group, whether they keep their tab's
//! title current, whether they're told to use Agent Screens, and what
//! Keep It Going types to one that has stopped.
//! Terminal follows them live. And brush's suggestions from Olo's model
//! (complete.rs).

use olo_kit::{ClickEvent, Font, ParentElement as _, Styled as _, WindowCx, div, metrics as m, px, widgets::Button};

use crate::form::Ui;

pub fn ui(ui: &mut Ui) {
    // (Side by side: Coding Agents, then worktrees and suggestions, so the
    // tallest of Suggestions' states still fits 1280 × 800.)
    ui.columns(coding_agents, |ui| {
        worktrees(ui);
        crate::complete::ui(ui);
    });
}

fn worktrees(ui: &mut Ui) {
    let t = ui.s.terminal.clone();
    ui.bbox("Git Worktrees", |ui| {
        let mono: Font = olo_kit::style::mono_font(olo_kit::FontWeight::NORMAL);
        ui.field("worktrees", &t.worktrees, |f| f.hint("~/worktrees").font(mono).name("New ones go in"), |s, v| s.terminal.worktrees = v);
        let base = t.worktrees.trim().trim_end_matches('/');
        let base = if base.is_empty() { "~/worktrees" } else { base };
        // The folder chosen in a panel too (Ben, 2026-11-17: "the
        // worktrees location should also have a button to pick the
        // folder"): opened at the one set, or the nearest folder above
        // it that there is.
        let set = base.to_owned();
        let choose = move |_: &ClickEvent, _: &mut WindowCx| {
            let home = std::path::PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
            let mut at = match set.strip_prefix("~") {
                Some(rest) => home.join(rest.trim_start_matches('/')),
                None => std::path::PathBuf::from(&set),
            };
            while !at.is_dir() && at.pop() {}
            // (The field shows it: without the keyboard, it follows
            // the setting.)
            olo_kit::file_panel::show("Choose Where Worktrees Go", olo_kit::file_panel::FilePanel::folder(at), move |a| {
                if let olo_kit::file_panel::PanelResult::Chosen(path) = a.result {
                    let folder = tilde(&path, &home);
                    crate::prefs().edit_as("", "Worktrees", move |s| s.terminal.worktrees = folder);
                }
            });
        };
        let enabled = ui.enabled();
        ui.add(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().child(ui.control_of("worktrees").label("New ones go in:").divider(100.0)))
                .child(Button::new("Choose…").id("worktrees-choose").enabled(enabled).on_click(choose)),
        );
        ui.note(format!("Each repository gets a folder in there: olo’s worktree fix-tabs goes in {base}/olo/fix-tabs."));
    });
}

fn coding_agents(ui: &mut Ui) {
    let t = ui.s.terminal.clone();
    ui.bbox("Coding Agents", |ui| {
        ui.check("Tell coding agents about the repositories in their tab group", t.agent_context, |s, v| s.terminal.agent_context = v);
        ui.note(
            "Claude Code and Codex started in a tab of a group whose tabs are in more than one repository are told the group’s name and those repositories and worktrees, theirs marked. Codex asks you once to trust Terminal’s hook (/hooks).",
        );
        ui.space(m::TIGHT);
        ui.check("Let coding agents keep their tab’s title current", t.agent_titles, |s, v| s.terminal.agent_titles = v);
        ui.note("Claude Code and Codex get a tool to title their tab with what they’re working on, and are asked to keep it current. They see the tool from their next start.");
        ui.space(m::TIGHT);
        ui.check("Tell coding agents to use Olo’s Agent Screens", t.agent_screens, |s, v| s.terminal.agent_screens = v);
        ui.note("Claude Code and Codex are told, as they start, to test and use desktop apps and web pages on screens of their own that you can watch and take over, never on yours, and how: Olo’s Chrome included, so they never start one on your screen.");
        // What Keep It Going types (Ben, 2026-11-08: "make the keep going
        // message configurable, by default it should be "Please continue"
        // … as well as revert back to the default").
        ui.space(m::TIGHT);
        let default = olo_protocol::TerminalSettings::KEEP_GOING_SAYS;
        let field = ui.field("keep-going-says", &t.keep_going_says, |f| f.hint(default).name("Keep It Going says"), |s, v| s.terminal.keep_going_says = v);
        let changed = t.keep_going_says != default;
        let set_default = ui.edit_click(move |s| s.terminal.keep_going_says = default.into());
        // (The field too: it keeps what's typed while it has the keyboard.)
        let shown = field;
        let use_default = move |e: &ClickEvent, cx: &mut WindowCx| {
            shown.update(cx, |f, cx| f.set_text(default.to_owned(), cx));
            set_default(e, cx);
        };
        let enabled = ui.enabled();
        ui.add(
            div()
                .flex()
                .flex_row()
                .items_center()
                .gap(px(m::SPACING))
                .child(div().flex_1().min_w_0().child(ui.control_of("keep-going-says").label("Keep It Going says:").divider(130.0)))
                .child(Button::new("Use Default").id("keep-going-default").enabled(enabled && changed).on_click(use_default)),
        );
        ui.note("Keep It Going, in a tab’s menu, types this to the coding agent in the tab when it has stopped: after two quiet minutes, at most once an hour. Left empty, it says “Please continue”.");
    });
}

/// `path` as the setting has it: under the home folder, from `~`.
fn tilde(path: &std::path::Path, home: &std::path::Path) -> String {
    match path.strip_prefix(home) {
        Ok(rest) if !home.as_os_str().is_empty() && rest.as_os_str().is_empty() => "~".into(),
        Ok(rest) if !home.as_os_str().is_empty() => format!("~/{}", rest.display()),
        _ => path.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    #[test]
    fn a_folder_under_home_is_written_from_the_tilde() {
        let home = Path::new("/home/mira");
        assert_eq!(super::tilde(Path::new("/home/mira/src/trees"), home), "~/src/trees");
        assert_eq!(super::tilde(Path::new("/home/mira"), home), "~");
        assert_eq!(super::tilde(Path::new("/srv/trees"), home), "/srv/trees");
        assert_eq!(super::tilde(Path::new("/home/mirabel/trees"), home), "/home/mirabel/trees");
        assert_eq!(super::tilde(Path::new("/srv/trees"), Path::new("")), "/srv/trees");
    }
}
