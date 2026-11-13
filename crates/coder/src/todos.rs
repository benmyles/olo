//! The task list: a tree kept as one ordered list, each task after its
//! parent and its subtasks right after it (pre-order), so it reads, saves
//! and sends as a flat list and still nests. The agent keeps it (whole,
//! with `set_todos`, or a few statuses at a time with `update_todos`);
//! the person edits it a task at a time (`TodoEdit`), each edit applied to
//! the list as it is then, so neither undoes the other's work, and said
//! to the agent in words (`describe`).
use crate::model::{Todo, TodoEdit};
use anyhow::{Context, Result, ensure};
use std::collections::{HashMap, HashSet};

/// The states a task can be in.
pub const STATUSES: [&str; 4] = ["pending", "in_progress", "done", "blocked"];
/// The most tasks a list holds, and how deep it nests.
pub const MOST: usize = 500;
pub const DEEPEST: usize = 6;

/// `todos` in tree order: each task's subtasks right after it, in the
/// order given (an agent may list a subtask before its parent).
pub fn arrange(todos: Vec<Todo>) -> Result<Vec<Todo>> {
    let ids: HashSet<&str> = todos.iter().map(|t| t.id.as_str()).collect();
    for t in &todos {
        if let Some(p) = &t.parent {
            ensure!(ids.contains(p.as_str()), "task {} names a parent that isn't in the list: {p}", t.id);
        }
    }
    let mut children: HashMap<Option<&str>, Vec<usize>> = HashMap::new();
    for (i, t) in todos.iter().enumerate() {
        children.entry(t.parent.as_deref()).or_default().push(i);
    }
    let mut order = Vec::with_capacity(todos.len());
    let mut stack: Vec<usize> = children.get(&None).map(|v| v.iter().rev().copied().collect()).unwrap_or_default();
    let mut seen = HashSet::new();
    while let Some(i) = stack.pop() {
        if !seen.insert(i) {
            continue;
        }
        order.push(i);
        if let Some(kids) = children.get(&Some(todos[i].id.as_str())) {
            stack.extend(kids.iter().rev().copied());
        }
    }
    // What isn't reached from the top is in a loop of parents.
    ensure!(order.len() == todos.len(), "the tasks' parents make a loop");
    let mut slots: Vec<Option<Todo>> = todos.into_iter().map(Some).collect();
    Ok(order.into_iter().map(|i| slots[i].take().unwrap()).collect())
}

/// Whether `todos` is a well-made list: ids unique and present, known
/// statuses, each task after its parent with its subtasks right after it,
/// within the size and depth limits.
pub fn validate(todos: &[Todo]) -> Result<()> {
    ensure!(todos.len() <= MOST, "a task list holds at most {MOST} tasks");
    let mut ids = HashSet::new();
    let mut path: Vec<&str> = Vec::new();
    for t in todos {
        ensure!(!t.id.is_empty() && t.id.len() <= 64 && ids.insert(&t.id), "task ids must be unique, nonempty and short");
        ensure!(!t.text.trim().is_empty() && t.text.len() <= 2000, "a task says what it is in at most 2000 bytes");
        ensure!(STATUSES.contains(&t.status.as_str()), "a task's status is one of {}", STATUSES.join(", "));
        match &t.parent {
            None => path.clear(),
            Some(p) => {
                let at = path.iter().rposition(|a| a == p).with_context(|| format!("task {} must come right after its parent {p} and its other subtasks", t.id))?;
                path.truncate(at + 1);
            }
        }
        path.push(&t.id);
        ensure!(path.len() <= DEEPEST, "tasks nest at most {DEEPEST} deep");
    }
    Ok(())
}

/// Where task `i`'s subtasks end (the index after its last descendant).
pub fn subtree_end(todos: &[Todo], i: usize) -> usize {
    let mut inside: HashSet<&str> = HashSet::from([todos[i].id.as_str()]);
    let mut end = i + 1;
    while end < todos.len() && todos[end].parent.as_deref().is_some_and(|p| inside.contains(p)) {
        inside.insert(&todos[end].id);
        end += 1;
    }
    end
}

/// How deep task `i` is (0: at the top).
pub fn depth(todos: &[Todo], i: usize) -> usize {
    let mut d = 0;
    let mut parent = todos[i].parent.as_deref();
    while let Some(p) = parent {
        d += 1;
        parent = todos.iter().find(|t| t.id == p).and_then(|t| t.parent.as_deref());
        if d > DEEPEST {
            break;
        }
    }
    d
}

fn find(todos: &[Todo], id: &str) -> Result<usize> {
    todos.iter().position(|t| t.id == id).with_context(|| format!("there's no task {id} (it may have just been removed)"))
}

fn quoted(todos: &[Todo], id: &str) -> String {
    todos.iter().find(|t| t.id == id).map_or_else(|| id.to_owned(), |t| format!("“{}”", t.text))
}

fn fresh_id(todos: &[Todo]) -> String {
    let n = todos.iter().filter_map(|t| t.id.strip_prefix('t')?.parse::<u64>().ok()).max().unwrap_or(0);
    format!("t{}", n + 1)
}

/// Where a task under `parent`, after the sibling `after` or before the
/// sibling `before`, goes.
fn place(todos: &[Todo], parent: Option<&str>, after: Option<&str>, before: Option<&str>) -> Result<usize> {
    if let Some(b) = before {
        let i = find(todos, b)?;
        ensure!(todos[i].parent.as_deref() == parent, "the task to go before isn't under the same parent");
        return Ok(i);
    }
    if let Some(a) = after {
        let i = find(todos, a)?;
        ensure!(todos[i].parent.as_deref() == parent, "the task to go after isn't under the same parent");
        return Ok(subtree_end(todos, i));
    }
    Ok(match parent {
        Some(p) => subtree_end(todos, find(todos, p)?),
        None => todos.len(),
    })
}

/// Applies the person's `edit` to `todos`, saying what it did (for the
/// agent, and the conversation).
pub fn apply(todos: &mut Vec<Todo>, edit: &TodoEdit) -> Result<String> {
    let said = match edit {
        TodoEdit::Add { text, parent, after, before, status } => {
            let text = text.trim();
            ensure!(!text.is_empty(), "a task says what it is");
            let at = place(todos, parent.as_deref(), after.as_deref(), before.as_deref())?;
            let id = fresh_id(todos);
            let status = status.clone().unwrap_or_else(|| "pending".into());
            todos.insert(at, Todo { id, text: text.into(), status, evidence: String::new(), parent: parent.clone() });
            match parent {
                Some(p) => format!("added “{text}” under {}", quoted(todos, p)),
                None => format!("added “{text}”"),
            }
        }
        TodoEdit::Remove { id } => {
            let i = find(todos, id)?;
            let end = subtree_end(todos, i);
            let gone: Vec<Todo> = todos.drain(i..end).collect();
            match gone.len() - 1 {
                0 => format!("removed “{}”", gone[0].text),
                1 => format!("removed “{}” and its subtask", gone[0].text),
                n => format!("removed “{}” and its {n} subtasks", gone[0].text),
            }
        }
        TodoEdit::Change { id, text, status } => {
            let i = find(todos, id)?;
            let mut parts = Vec::new();
            if let Some(text) = text.as_deref().map(str::trim).filter(|t| *t != todos[i].text) {
                ensure!(!text.is_empty(), "a task says what it is");
                parts.push(format!("renamed “{}” to “{text}”", todos[i].text));
                todos[i].text = text.into();
            }
            if let Some(status) = status.as_deref().filter(|s| *s != todos[i].status) {
                ensure!(STATUSES.contains(&status), "a task's status is one of {}", STATUSES.join(", "));
                parts.push(format!("marked “{}” {}", todos[i].text, status.replace('_', " ")));
                todos[i].status = status.into();
            }
            if parts.is_empty() {
                return Ok(String::new());
            }
            parts.join("; ")
        }
        TodoEdit::Move { id, parent, after, before } => {
            let i = find(todos, id)?;
            let end = subtree_end(todos, i);
            if let Some(p) = parent {
                ensure!(!todos[i..end].iter().any(|t| &t.id == p), "a task can't go under itself");
            }
            let mut block: Vec<Todo> = todos.drain(i..end).collect();
            let was = std::mem::replace(&mut block[0].parent, parent.clone());
            let at = match place(todos, parent.as_deref(), after.as_deref(), before.as_deref()) {
                Ok(at) => at,
                Err(e) => {
                    // Back where it was.
                    block[0].parent = was;
                    todos.splice(i..i, block);
                    return Err(e);
                }
            };
            let text = block[0].text.clone();
            todos.splice(at..at, block);
            match parent {
                Some(p) => format!("moved “{text}” under {}", quoted(todos, p)),
                None if after.is_none() && before.is_none() => format!("moved “{text}” to the end"),
                None => format!("moved “{text}”"),
            }
        }
    };
    validate(todos)?;
    Ok(said)
}

/// What the agent is told when the person edits the list.
pub fn notice(said: &[String]) -> String {
    format!(
        "[Olo Coder: the person edited the task list] {}. The task list in the session state is now current: work from it, keep it up to date, and say so if you think a change is wrong.",
        said.join("; ")
    )
}

/// The agent's quick update: statuses, wording or evidence of tasks
/// already in the list.
pub fn update(todos: &mut [Todo], changes: &[serde_json::Value]) -> Result<usize> {
    let mut n = 0;
    for c in changes {
        let id = c["id"].as_str().context("each change names a task id")?;
        let i = todos.iter().position(|t| t.id == id).with_context(|| format!("there's no task {id}; use set_todos to add tasks"))?;
        if let Some(status) = c["status"].as_str() {
            ensure!(STATUSES.contains(&status), "a task's status is one of {}", STATUSES.join(", "));
            todos[i].status = status.into();
        }
        if let Some(text) = c["text"].as_str().filter(|t| !t.trim().is_empty()) {
            todos[i].text = text.trim().into();
        }
        if let Some(evidence) = c["evidence"].as_str() {
            todos[i].evidence = evidence.into();
        }
        n += 1;
    }
    validate(todos)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn todo(id: &str, parent: Option<&str>) -> Todo {
        Todo { id: id.into(), text: format!("task {id}"), status: "pending".into(), evidence: String::new(), parent: parent.map(Into::into) }
    }

    fn ids(todos: &[Todo]) -> String {
        todos.iter().map(|t| t.id.as_str()).collect::<Vec<_>>().join(" ")
    }

    #[test]
    fn arranges_subtasks_under_their_parents_and_refuses_loops() {
        let list = arrange(vec![todo("c", Some("a")), todo("a", None), todo("b", None), todo("d", Some("c"))]).unwrap();
        assert_eq!(ids(&list), "a c d b");
        validate(&list).unwrap();
        assert!(arrange(vec![todo("a", Some("b")), todo("b", Some("a"))]).is_err());
        assert!(arrange(vec![todo("a", Some("missing"))]).is_err());
        assert!(validate(&[todo("a", None), todo("b", None), todo("c", Some("a"))]).is_err(), "a subtask must follow its parent's subtree");
    }

    #[test]
    fn the_persons_edits_apply_to_the_list_as_it_is_and_say_what_they_did() {
        let mut list = vec![todo("t1", None), todo("t2", Some("t1")), todo("t3", None)];
        let said = apply(&mut list, &TodoEdit::Add { text: "Write docs".into(), parent: Some("t1".into()), after: None, before: None, status: None }).unwrap();
        assert_eq!(ids(&list), "t1 t2 t4 t3");
        assert_eq!(said, "added “Write docs” under “task t1”");
        let said = apply(&mut list, &TodoEdit::Change { id: "t2".into(), text: None, status: Some("done".into()) }).unwrap();
        assert_eq!(said, "marked “task t2” done");
        apply(&mut list, &TodoEdit::Move { id: "t1".into(), parent: None, after: Some("t3".into()), before: None }).unwrap();
        assert_eq!(ids(&list), "t3 t1 t2 t4");
        assert!(apply(&mut list, &TodoEdit::Move { id: "t1".into(), parent: Some("t2".into()), after: None, before: None }).is_err(), "not under itself");
        assert_eq!(ids(&list), "t3 t1 t2 t4", "a refused move leaves the list as it was");
        apply(&mut list, &TodoEdit::Move { id: "t3".into(), parent: Some("t1".into()), after: Some("t2".into()), before: None }).unwrap();
        assert_eq!(ids(&list), "t1 t2 t3 t4");
        apply(&mut list, &TodoEdit::Move { id: "t4".into(), parent: Some("t1".into()), after: None, before: Some("t2".into()) }).unwrap();
        assert_eq!(ids(&list), "t1 t4 t2 t3");
        let said = apply(&mut list, &TodoEdit::Remove { id: "t1".into() }).unwrap();
        assert_eq!(said, "removed “task t1” and its 3 subtasks");
        assert!(list.is_empty());
        assert!(apply(&mut list, &TodoEdit::Remove { id: "t1".into() }).is_err());
    }
}
