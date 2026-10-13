//! Claude Code's managed worktrees: `<repo>/.claude/worktrees/<name>`.

/// The path segments that mark a Claude-managed worktree below a repository.
const MARKER: &str = "/.claude/worktrees/";

/// A Claude-managed worktree: the repository it belongs to and its name, the
/// value `claude --worktree` takes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Worktree {
    pub repo_root: String,
    pub name: String,
}

impl Worktree {
    /// The worktree a stored cwd sits in: the cwd itself or any directory
    /// beneath `<repo_root>/.claude/worktrees/<name>`. `None` for a relative
    /// path, a missing name or any other place.
    pub fn from_cwd(cwd: &str) -> Option<Self> {
        if !cwd.starts_with('/') {
            return None;
        }
        let at = cwd.find(MARKER)?;
        let repo_root = cwd.get(..at)?;
        let rest = cwd.get(at.checked_add(MARKER.len())?..)?;
        let name = rest.split('/').next().filter(|name| !name.is_empty())?;
        Some(Self {
            repo_root: if repo_root.is_empty() {
                "/".to_owned()
            } else {
                repo_root.to_owned()
            },
            name: name.to_owned(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(cwd: &str) -> Option<(String, String)> {
        Worktree::from_cwd(cwd).map(|w| (w.repo_root, w.name))
    }

    fn pair(root: &str, name: &str) -> Option<(String, String)> {
        Some((root.to_owned(), name.to_owned()))
    }

    #[test]
    fn a_worktree_directory_gives_its_repo_and_name() {
        assert_eq!(
            found("/home/u/repo/.claude/worktrees/deep-wishing-squirrel"),
            pair("/home/u/repo", "deep-wishing-squirrel")
        );
    }

    #[test]
    fn a_repo_that_is_itself_a_worktree_matches_the_first_marker() {
        assert_eq!(
            found("/r/.claude/worktrees/a/.claude/worktrees/b"),
            pair("/r", "a")
        );
    }

    #[test]
    fn a_subdirectory_resolves_to_the_worktree() {
        assert_eq!(found("/r/.claude/worktrees/wt/scripts/x"), pair("/r", "wt"));
    }

    #[test]
    fn spaces_are_kept() {
        assert_eq!(
            found("/home/my user/my repo/.claude/worktrees/odd name/src"),
            pair("/home/my user/my repo", "odd name")
        );
    }

    #[test]
    fn a_trailing_slash_is_ignored() {
        assert_eq!(found("/r/.claude/worktrees/wt/"), pair("/r", "wt"));
    }

    #[test]
    fn a_repo_at_the_filesystem_root_keeps_a_root() {
        assert_eq!(found("/.claude/worktrees/wt"), pair("/", "wt"));
    }

    #[test]
    fn anything_else_is_not_a_worktree() {
        for cwd in [
            "/home/u/repo",
            "/home/u/repo/.claude",
            "/home/u/repo/.claude/worktrees",
            "/home/u/repo/.claude/worktrees/",
            "/home/u/repo/.claude/worktrees//x",
            "/.claude/worktrees",
            "relative/.claude/worktrees/wt",
            ".claude/worktrees/wt",
            "/r/x.claude/worktrees/wt",
            "",
        ] {
            assert_eq!(found(cwd), None, "{cwd:?}");
        }
    }
}
