//! A file's change as a coding agent's step reports it: `{file, patch, additions, deletions,
//! status}`, from the file before and after or from hunks the agent made itself.

use kuvo_store::diff::{from_patch as count_patch, unified};
use serde_json::{Value, json};

/// The change from `before` to `after` (`None`: the file wasn't there, or isn't now).
pub fn file_change(path: &str, before: Option<&str>, after: Option<&str>) -> Value {
    let diff = unified(before.unwrap_or_default(), after.unwrap_or_default(), 3);
    let status = match (before, after) {
        (None, _) => "added",
        (_, None) => "deleted",
        _ => "modified",
    };
    json!({
        "file": path, "patch": header(path) + &diff.patch,
        "additions": diff.additions, "deletions": diff.deletions, "status": status,
    })
}

/// A unified diff someone else made, with its lines counted.
pub fn patch_change(path: &str, patch: &str, status: Option<&str>) -> Value {
    let counts = count_patch(patch);
    let headed = if patch.starts_with("---") || patch.starts_with("diff ") {
        patch.to_owned()
    } else {
        header(path) + patch
    };
    json!({
        "file": path, "patch": headed, "additions": counts.additions,
        "deletions": counts.deletions, "status": status.unwrap_or("modified"),
    })
}

fn header(path: &str) -> String {
    if path.starts_with('/') {
        format!("--- {path}\n+++ {path}\n")
    } else {
        format!("--- a/{path}\n+++ b/{path}\n")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_a_change_and_an_agents_own_hunks() {
        let change = file_change("/w/a.ts", Some("const a = 1\n"), Some("const a = 2\n"));
        assert_eq!(
            change,
            json!({"file":"/w/a.ts","patch":"--- /w/a.ts\n+++ /w/a.ts\n@@ -1,1 +1,1 @@\n-const a = 1\n+const a = 2\n","additions":1,"deletions":1,"status":"modified"})
        );
        assert_eq!(file_change("b.ts", None, Some("x\n"))["status"], "added");
        let hunks = patch_change("a.ts", "@@ -1,1 +1,2 @@\n-a\n+b\n+c\n", None);
        assert_eq!(hunks["patch"], "--- a/a.ts\n+++ b/a.ts\n@@ -1,1 +1,2 @@\n-a\n+b\n+c\n");
        assert_eq!((hunks["additions"].as_u64(), hunks["deletions"].as_u64()), (Some(2), Some(1)));
    }
}
