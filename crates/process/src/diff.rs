use similar::{ChangeTag, TextDiff};

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct DiffResult {
    pub added: Vec<String>,
    pub removed: Vec<String>,
}

impl DiffResult {
    pub fn is_empty(&self) -> bool {
        self.added.is_empty() && self.removed.is_empty()
    }
}

pub fn diff(old: &str, new: &str) -> DiffResult {
    let mut out = DiffResult::default();
    for change in TextDiff::from_lines(old, new).iter_all_changes() {
        let line = change.value().trim_end_matches('\n').to_string();
        match change.tag() {
            ChangeTag::Insert => out.added.push(line),
            ChangeTag::Delete => out.removed.push(line),
            ChangeTag::Equal => {}
        }
    }
    out
}
