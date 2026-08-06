//! Git-spec resolution and materialization via the gix crate (pure Rust, no
//! system git), mirroring pnpm's `resolving-git-resolver` + `git-fetcher`.
//!
//! Strategy: instead of `git ls-remote` for ref pinning, we clone (full fetch)
//! once and resolve the ref locally with `rev_parse_single` (which supports
//! tags, branches, `HEAD` and short SHA prefixes). The same clone's commit
//! tree is then walked to materialize the pinned content. A full 40-hex
//! commit spec needs no network at all.

use std::path::Path;
use std::sync::atomic::AtomicBool;

use crate::error::{OhpmError, Result};
use crate::install::semver::semver_max_satisfying;

/// A parsed git fragment after the `#` (mirrors pnpm's fragment syntax).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GitQuery {
    /// No fragment — `HEAD`.
    Head,
    /// A full 40-hex commit SHA (no network needed).
    Commit(String),
    /// A 7–39 hex SHA prefix (resolved to the unique matching commit).
    Partial(String),
    /// A branch or tag name (`main`, `v1.0.0`, `heads/canary`).
    Ref(String),
    /// `semver:<range>` — the max satisfying tag.
    Semver(String),
    /// `#path:<subdir>` (optionally combined with another fragment via `&`).
    Path {
        sub: String,
        inner: Box<GitQuery>,
    },
}

/// Parse a git spec (`<url>#<fragment>`), mirroring pnpm's fragment grammar.
pub fn parse_git_spec(spec: &str) -> Result<GitQuery> {
    let frag = spec.split_once('#').map(|(_, f)| f).unwrap_or("");
    if frag.is_empty() {
        return Ok(GitQuery::Head);
    }
    if let Some(rest) = frag.strip_prefix("path:") {
        let (sub, inner) = rest.split_once('&').unwrap_or((rest, ""));
        let inner = if inner.is_empty() {
            GitQuery::Head
        } else {
            parse_fragment(inner)?
        };
        return Ok(GitQuery::Path {
            sub: sub.to_string(),
            inner: Box::new(inner),
        });
    }
    if let Some((inner, sub)) = frag.split_once("&path:") {
        return Ok(GitQuery::Path {
            sub: sub.to_string(),
            inner: Box::new(parse_fragment(inner)?),
        });
    }
    parse_fragment(frag)
}

fn parse_fragment(frag: &str) -> Result<GitQuery> {
    if frag.len() == 40 && frag.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(GitQuery::Commit(frag.to_string()));
    }
    if (7..40).contains(&frag.len()) && frag.chars().all(|c| c.is_ascii_hexdigit()) {
        return Ok(GitQuery::Partial(frag.to_string()));
    }
    if let Some(range) = frag.strip_prefix("semver:") {
        return Ok(GitQuery::Semver(range.to_string()));
    }
    Ok(GitQuery::Ref(frag.to_string()))
}

/// The `#path:` subdirectory of a query, if any.
pub fn sub_dir_of(query: &GitQuery) -> Option<&str> {
    match query {
        GitQuery::Path { sub, .. } => Some(sub.as_str()),
        _ => None,
    }
}

/// `prepare_clone` + `fetch_only` (full fetch of all refs).
pub fn fetch_repo(url: &str, tmp: &Path) -> Result<gix::Repository> {
    let mut prep = gix::prepare_clone(url, tmp).map_err(|e| {
        OhpmError::git_ls_remote_failed(url, &e.to_string())
    })?;
    let (repo, _outcome) = prep
        .fetch_only(gix::progress::Discard, &AtomicBool::new(false))
        .map_err(|e| OhpmError::git_ls_remote_failed(url, &e.to_string()))?;
    Ok(repo)
}

/// Resolve a query to a commit hex inside a cloned repository, following
/// pnpm's ref priority (full SHA → prefix → ref → refs/<ref> →
/// refs/tags/<ref>^{} → refs/tags/<ref> → refs/heads/<ref>; `HEAD`; semver
/// over tags).
pub fn resolve_commit_in(repo: &gix::Repository, query: &GitQuery) -> Result<String> {
    match query {
        GitQuery::Commit(sha) => Ok(sha.clone()),
        GitQuery::Partial(prefix) => rev_parse(repo, prefix),
        GitQuery::Head => rev_parse(repo, "HEAD"),
        GitQuery::Ref(r) => {
            let candidates = [
                r.as_str(),
                &format!("refs/{r}"),
                &format!("refs/tags/{r}^{{}}"),
                &format!("refs/tags/{r}"),
                &format!("refs/heads/{r}"),
            ];
            for cand in candidates {
                if let Ok(sha) = rev_parse(repo, cand) {
                    return Ok(sha);
                }
            }
            Err(OhpmError::git_ref_not_found(
                &repo_path(repo),
                r,
            ))
        }
        GitQuery::Semver(range) => {
            let tags = tag_names(repo)?;
            let matching = semver_max_satisfying(&tags, range);
            match matching {
                Some(normalized) => {
                    // The matcher normalizes versions (drops a "v" prefix) —
                    // find the original tag name again.
                    let tag = tags
                        .iter()
                        .find(|t| {
                            node_semver::Version::parse(t)
                                .map(|v| v.to_string())
                                .ok()
                                .as_deref()
                                == Some(normalized.as_str())
                        })
                        .cloned()
                        .unwrap_or(normalized);
                    rev_parse(repo, &format!("refs/tags/{tag}^{{}}"))
                }
                None => Err(OhpmError::git_semver_no_match(
                    &repo_path(repo),
                    range,
                )),
            }
        }
        GitQuery::Path { inner, .. } => resolve_commit_in(repo, inner),
    }
}

/// `rev_parse_single` with error mapping (ambiguous prefixes → GitAmbiguousRef).
fn rev_parse(repo: &gix::Repository, spec: &str) -> Result<String> {
    let id = repo.rev_parse_single(spec).map_err(|e| {
        let msg = e.to_string();
        if msg.to_lowercase().contains("ambiguous") {
            OhpmError::git_ambiguous_ref(&repo_path(repo), spec, 2)
        } else {
            OhpmError::git_ref_not_found(&repo_path(repo), spec)
        }
    })?;
    Ok(id.to_string())
}

fn repo_path(repo: &gix::Repository) -> String {
    repo.path().display().to_string()
}

/// All `refs/tags/<name>` names (deduplicated, `^{}` suffix stripped).
fn tag_names(repo: &gix::Repository) -> Result<Vec<String>> {
    let mut out = Vec::new();
    let refs = repo.references().map_err(|e| OhpmError::git_ls_remote_failed(&repo_path(repo), &e.to_string()))?;
    for r in refs.all().map_err(|e| OhpmError::git_ls_remote_failed(&repo_path(repo), &e.to_string()))? {
        let r = r.map_err(|e| OhpmError::git_ls_remote_failed(&repo_path(repo), &e.to_string()))?;
        let name = r.name().as_bstr().to_string();
        if let Some(tag) = name.strip_prefix("refs/tags/") {
            let tag = tag.strip_suffix("^{}").unwrap_or(tag).to_string();
            if !out.contains(&tag) {
                out.push(tag);
            }
        }
    }
    Ok(out)
}

/// Materialize the pinned commit's tree (optionally a subdirectory) into
/// `dest`, then delete the clone's `.git`. The checkout is a manual tree walk
/// (the gix checkout API cannot target an arbitrary pinned commit).
pub fn materialize_commit_in(
    repo: &gix::Repository,
    commit: &str,
    sub_dir: Option<&str>,
    dest: &Path,
) -> Result<()> {
    let id = gix::ObjectId::from_hex(commit.as_bytes()).map_err(|e| {
        OhpmError::git_checkout_failed(&repo_path(repo), commit, &e.to_string())
    })?;
    let commit_obj = repo.find_commit(id).map_err(|e| {
        OhpmError::git_checkout_failed(&repo_path(repo), commit, &e.to_string())
    })?;
    let tree_id = commit_obj
        .tree_id()
        .map_err(|e| OhpmError::git_checkout_failed(&repo_path(repo), commit, &e.to_string()))?;
    let tree = repo.find_tree(tree_id).map_err(|e| {
        OhpmError::git_checkout_failed(&repo_path(repo), commit, &e.to_string())
    })?;
    let tree = match sub_dir {
        Some(sub) => descend(repo, &tree, sub).map_err(|e| {
            OhpmError::git_checkout_failed(&repo_path(repo), commit, &e.to_string())
        })?,
        None => tree,
    };
    write_tree(repo, &tree, dest).map_err(|e| {
        OhpmError::git_checkout_failed(&repo_path(repo), commit, &e.to_string())
    })?;
    // Drop the clone metadata: the store must not contain `.git`.
    let _ = std::fs::remove_dir_all(repo.git_dir());
    let _ = std::fs::remove_file(dest.join(".git"));
    Ok(())
}

/// Descend into a subdirectory of the tree.
fn descend<'a>(repo: &'a gix::Repository, tree: &gix::Tree<'a>, sub: &str) -> Result<gix::Tree<'a>> {
    let mut current = tree.clone();
    let not_found = |detail: &str| {
        OhpmError::git_ref_not_found(&repo.path().display().to_string(), detail)
    };
    for part in sub.split('/').filter(|p| !p.is_empty()) {
        let mut found = None;
        for entry in current.iter() {
            let entry = entry.map_err(|e| not_found(&e.to_string()))?;
            if entry.filename().to_string() == part {
                if entry.mode().kind() != gix::object::tree::EntryKind::Tree {
                    return Err(not_found(sub));
                }
                found = Some(repo.find_tree(entry.id()).map_err(|e| not_found(&e.to_string()))?);
                break;
            }
        }
        current = found.ok_or_else(|| not_found(sub))?;
    }
    Ok(current)
}

/// Recursively write a tree into `dest` (blobs as files, links as symlinks,
/// submodules skipped).
fn write_tree(repo: &gix::Repository, tree: &gix::Tree<'_>, dest: &Path) -> std::result::Result<(), Box<dyn std::error::Error + Send + Sync>> {
    std::fs::create_dir_all(dest)?;
    for entry in tree.iter() {
        let entry = entry?;
        let name = entry.filename().to_string();
        let out = dest.join(&name);
        match entry.mode().kind() {
            gix::object::tree::EntryKind::Tree => {
                let t = repo.find_tree(entry.id())?;
                write_tree(repo, &t, &out)?;
            }
            gix::object::tree::EntryKind::Blob | gix::object::tree::EntryKind::BlobExecutable => {
                let obj = repo.find_object(entry.id())?;
                let blob = obj.into_blob();
                std::fs::write(&out, &blob.data)?;
            }
            gix::object::tree::EntryKind::Link => {
                let obj = repo.find_object(entry.id())?;
                let blob = obj.into_blob();
                let target = String::from_utf8_lossy(&blob.data);
                #[cfg(unix)]
                std::os::unix::fs::symlink(target.as_ref(), &out)?;
                #[cfg(not(unix))]
                std::fs::write(&out, target.as_bytes())?;
            }
            gix::object::tree::EntryKind::Commit => {
                // Submodule — skipped, like the reference's clone handling.
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    /// Build a fixture repo with the system git (tests may use git; the
    /// product code never does).
    struct Fixture {
        dir: tempfile::TempDir,
        pub url: String,
    }

    impl Fixture {
        fn new() -> Fixture {
            let dir = tempfile::TempDir::new().unwrap();
            let repo = dir.path().join("repo");
            std::fs::create_dir_all(&repo).unwrap();
            run(repo.as_path(), &["init", "-q"]);
            Fixture {
                dir,
                url: format!("file://{}", repo.display()),
            }
        }

        fn write(&self, rel: &str, content: &str) {
            let repo = self.dir.path().join("repo");
            let p = repo.join(rel);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, content).unwrap();
        }

        fn commit(&self, msg: &str) {
            let repo = self.dir.path().join("repo");
            run(repo.as_path(), &["add", "."]);
            run(repo.as_path(), &["commit", "-q", "-m", msg]);
        }

        fn tag(&self, tag: &str) {
            let repo = self.dir.path().join("repo");
            run(repo.as_path(), &["tag", tag]);
        }

    }

    fn run(dir: &Path, args: &[&str]) {
        let status = Command::new("git").args(args).current_dir(dir).status().unwrap();
        assert!(status.success(), "git {args:?} failed");
    }

    #[test]
    fn parse_fragments() {
        assert_eq!(parse_git_spec("git+https://x/y.git").unwrap(), GitQuery::Head);
        assert_eq!(
            parse_git_spec("git+https://x/y.git#v1.0.0").unwrap(),
            GitQuery::Ref("v1.0.0".into())
        );
        assert_eq!(
            parse_git_spec("git+https://x/y.git#main").unwrap(),
            GitQuery::Ref("main".into())
        );
        assert_eq!(
            parse_git_spec("git+https://x/y.git#0123456789abcdef0123456789abcdef01234567").unwrap(),
            GitQuery::Commit("0123456789abcdef0123456789abcdef01234567".into())
        );
        assert_eq!(
            parse_git_spec("git+https://x/y.git#0123456789").unwrap(),
            GitQuery::Partial("0123456789".into())
        );
        assert_eq!(
            parse_git_spec("git+https://x/y.git#semver:^1.0.0").unwrap(),
            GitQuery::Semver("^1.0.0".into())
        );
        assert_eq!(
            parse_git_spec("git+https://x/y.git#beta&path:/packages/foo").unwrap(),
            GitQuery::Path {
                sub: "/packages/foo".into(),
                inner: Box::new(GitQuery::Ref("beta".into())),
            }
        );
        assert_eq!(
            parse_git_spec("git+https://x/y.git#path:/packages/foo").unwrap(),
            GitQuery::Path {
                sub: "/packages/foo".into(),
                inner: Box::new(GitQuery::Head),
            }
        );
    }

    #[test]
    fn resolve_and_materialize() {
        let fx = Fixture::new();
        fx.write("oh-package.json5", "{ name: \"foo\", version: \"1.0.0\" }\n");
        fx.write("src/a.ets", "export {}\n");
        fx.commit("init");
        fx.tag("v1.0.0");
        fx.write("src/b.ets", "export const b = 1\n");
        fx.commit("second");
        fx.tag("v1.1.0");

        let tmp = fx.dir.path().join("clone");
        let repo = fetch_repo(&fx.url, &tmp).unwrap();

        // Tag refs resolve.
        assert_eq!(
            resolve_commit_in(&repo, &GitQuery::Ref("v1.0.0".into())).unwrap(),
            resolve_commit_in(&repo, &GitQuery::Ref("v1.0.0".into())).unwrap()
        );
        let v110 = resolve_commit_in(&repo, &GitQuery::Ref("v1.1.0".into())).unwrap();
        let head = resolve_commit_in(&repo, &GitQuery::Head).unwrap();
        assert_eq!(v110, head, "HEAD == latest tag commit");

        // Branch name resolves.
        let master = resolve_commit_in(&repo, &GitQuery::Ref("master".into())).unwrap();
        assert_eq!(master, head);

        // Full SHA direct.
        assert_eq!(
            resolve_commit_in(&repo, &GitQuery::Commit(v110.clone())).unwrap(),
            v110
        );
        // Short SHA prefix.
        assert_eq!(
            resolve_commit_in(&repo, &GitQuery::Partial(v110[..10].into())).unwrap(),
            v110
        );

        // Semver picks the max matching tag.
        let semver = resolve_commit_in(&repo, &GitQuery::Semver("^1.0.0".into())).unwrap();
        assert_eq!(semver, v110);
        let err = resolve_commit_in(&repo, &GitQuery::Semver("^2.0.0".into())).unwrap_err();
        assert_eq!(err.code, "GitSemverNoMatch");

        // Unknown ref.
        let err = resolve_commit_in(&repo, &GitQuery::Ref("nope".into())).unwrap_err();
        assert_eq!(err.code, "GitRefNotFound");

        // Materialize the v1.0.0 content (only the first commit's files).
        let dest = fx.dir.path().join("out");
        materialize_commit_in(&repo, &resolve_commit_in(&repo, &GitQuery::Ref("v1.0.0".into())).unwrap(), None, &dest).unwrap();
        assert!(dest.join("oh-package.json5").is_file());
        assert!(dest.join("src/a.ets").is_file());
    }

    #[test]
    fn materialize_has_no_git_dir() {
        let fx = Fixture::new();
        fx.write("oh-package.json5", "{ name: \"foo\", version: \"1.0.0\" }\n");
        fx.commit("init");

        let tmp = fx.dir.path().join("clone");
        let repo = fetch_repo(&fx.url, &tmp).unwrap();
        let head = resolve_commit_in(&repo, &GitQuery::Head).unwrap();
        let dest = fx.dir.path().join("out");
        materialize_commit_in(&repo, &head, None, &dest).unwrap();
        assert!(!dest.join(".git").exists(), "no .git in the store dir");
    }
}
