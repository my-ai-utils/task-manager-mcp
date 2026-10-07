/// What a person typed, read as a repository.
///
/// `branch` and `path` are what the url ITSELF carried, which is only ever the case for a
/// `/tree/<branch>/<path>` url — the address you get by browsing to a folder on github.com and copying
/// the bar. That is the common way somebody names "this folder of that repo", and re-typing the two
/// halves into separate boxes afterwards is the kind of step people get wrong.
#[derive(Debug, Clone, PartialEq)]
pub struct ParsedRepoUrl {
    pub owner: String,
    pub repo: String,
    /// Empty when the url did not name one — the connection then follows the repository's default
    /// branch, resolved at pull time.
    pub branch: String,
    /// Empty when the url did not name one.
    pub path: String,
}

/// Read a repository out of whatever spelling of it arrived.
///
/// **Every form a person actually has to hand is accepted**, because the alternative is a validation
/// message telling somebody their own repository's address is wrong:
///
/// * `https://github.com/owner/repo`, with or without `.git`, with or without a trailing slash
/// * `git@github.com:owner/repo.git` — what the clone button offers under SSH
/// * `ssh://git@github.com/owner/repo.git`
/// * `owner/repo` — what a person types when they are not copying anything
/// * `https://github.com/owner/repo/tree/<branch>/<folder>` — the address bar, browsed to a folder
///
/// **Only github.com.** A host that is not github.com is refused by name rather than attempted: git
/// would clone an Enterprise host perfectly well, but there is nowhere on a connection to put one. It
/// carries an owner and a repo and no host, `workdir::repo_url` composes the remote as
/// `https://github.com/<owner>/<repo>.git` out of those two, and `git::auth_args` scopes the key to
/// `http.https://github.com/` so it is never handed to another host. Enterprise means carrying a host
/// through all three, not loosening the check here.
///
/// A branch containing a slash is genuinely ambiguous in a `/tree/` url — `tree/feature/x/docs` could be
/// branch `feature` and path `x/docs`, or branch `feature/x` and path `docs`, and the url carries
/// nothing that separates them. The first segment is taken as the branch, which is the reading that is
/// right for every branch that has no slash in it; the branch box is there to correct the rest.
pub fn parse_repo_url(src: &str) -> Result<ParsedRepoUrl, String> {
    let src = src.trim();

    if src.is_empty() {
        return Err("give the repository — a github.com url, or owner/repo".to_string());
    }

    // `git@github.com:owner/repo.git`. Rewritten into the path form rather than parsed separately, so
    // there is one set of segment rules below instead of two that can disagree.
    let rest = if let Some(rest) = src.strip_prefix("git@github.com:") {
        rest
    } else {
        let without_scheme = src
            .strip_prefix("https://")
            .or_else(|| src.strip_prefix("http://"))
            .or_else(|| src.strip_prefix("ssh://git@"))
            .or_else(|| src.strip_prefix("git://"))
            .unwrap_or(src);

        let without_scheme = without_scheme.strip_prefix("www.").unwrap_or(without_scheme);

        match without_scheme.strip_prefix("github.com/") {
            Some(rest) => rest,
            None => {
                // No host at all is the bare `owner/repo` form. A host that is not github.com is a
                // different product, and saying so is more use than "could not parse".
                if without_scheme.contains('.') && without_scheme.contains('/') {
                    let host = without_scheme.split('/').next().unwrap_or(without_scheme);

                    if host.contains('.') {
                        return Err(format!(
                            "'{host}' is not github.com — this connects to github.com repositories only"
                        ));
                    }
                }

                without_scheme
            }
        }
    };

    // A query or a fragment is decoration on an address somebody copied; neither names anything here.
    let rest = rest.split(['?', '#']).next().unwrap_or(rest);

    let mut segments = rest
        .split('/')
        .map(str::trim)
        .filter(|itm| !itm.is_empty());

    let owner = segments
        .next()
        .ok_or_else(|| format!("'{src}' does not name a repository — expected owner/repo"))?;

    let repo = segments
        .next()
        .ok_or_else(|| format!("'{src}' names an owner but no repository — expected owner/repo"))?;

    let repo = repo.strip_suffix(".git").unwrap_or(repo);

    if owner.is_empty() || repo.is_empty() {
        return Err(format!("'{src}' does not name a repository — expected owner/repo"));
    }

    // Everything past owner/repo. `tree` and `blob` are the two github.com puts there; anything else is
    // a page about the repository rather than a place in it, and is ignored.
    let mut branch = String::new();
    let mut path = String::new();

    if let Some(marker) = segments.next() {
        if marker == "tree" || marker == "blob" {
            if let Some(head) = segments.next() {
                branch = head.to_string();

                let tail: Vec<&str> = segments.collect();

                if !tail.is_empty() {
                    path = tail.join("/");
                }
            }
        }
    }

    Ok(ParsedRepoUrl {
        owner: owner.to_string(),
        repo: repo.to_string(),
        branch,
        path,
    })
}

/// Normalise a folder inside a repository, or say why it is not one.
///
/// Empty is legitimate and means the whole repository, which is why this cannot just be
/// `normalise_document_path` — that one refuses an empty path, correctly, because a document must be
/// called something and a folder need not be.
pub fn normalise_repo_path(src: &str) -> Result<String, String> {
    let src = src.trim().replace('\\', "/");
    let src = src.trim_matches('/');

    if src.is_empty() {
        return Ok(String::new());
    }

    let mut segments: Vec<&str> = Vec::new();

    for segment in src.split('/') {
        let segment = segment.trim();

        if segment.is_empty() {
            continue;
        }

        if segment == "." || segment == ".." {
            return Err(format!(
                "'{src}' contains '{segment}' — name the folder from the root of the repository"
            ));
        }

        segments.push(segment);
    }

    Ok(segments.join("/"))
}

/// Normalise a branch or tag, or say why it is not one.
///
/// Empty means the default branch. The rest is a light check rather than git's full refname grammar:
/// what this is guarding is a value about to be put in a url, so what matters is that it cannot escape
/// the path segment it belongs in.
pub fn normalise_branch(src: &str) -> Result<String, String> {
    let branch = src.trim().trim_matches('/');

    if branch.is_empty() {
        return Ok(String::new());
    }

    if branch.len() > 250 {
        return Err("that branch name is too long".to_string());
    }

    if branch.contains("..") || branch.contains(['?', '#', ' ', '\\']) {
        return Err(format!("'{branch}' is not a branch or tag name"));
    }

    Ok(branch.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parsed(owner: &str, repo: &str, branch: &str, path: &str) -> ParsedRepoUrl {
        ParsedRepoUrl {
            owner: owner.to_string(),
            repo: repo.to_string(),
            branch: branch.to_string(),
            path: path.to_string(),
        }
    }

    /// Every spelling of the clone button, plus the one people type from memory.
    #[test]
    fn the_forms_a_person_actually_has_to_hand_all_parse() {
        for src in [
            "https://github.com/MyJetTools/fl-url",
            "https://github.com/MyJetTools/fl-url/",
            "https://github.com/MyJetTools/fl-url.git",
            "http://github.com/MyJetTools/fl-url",
            "https://www.github.com/MyJetTools/fl-url",
            "git@github.com:MyJetTools/fl-url.git",
            "ssh://git@github.com/MyJetTools/fl-url.git",
            "MyJetTools/fl-url",
            "  MyJetTools/fl-url  ",
        ] {
            assert_eq!(
                parse_repo_url(src).unwrap(),
                parsed("MyJetTools", "fl-url", "", ""),
                "failed on {src}"
            );
        }
    }

    /// The whole reason the url is parsed rather than split into three boxes: browsing to a folder and
    /// copying the address bar is how a person names one.
    #[test]
    fn a_tree_url_carries_the_branch_and_the_folder_with_it() {
        assert_eq!(
            parse_repo_url("https://github.com/MyJetTools/fl-url/tree/main/src/non_wasm").unwrap(),
            parsed("MyJetTools", "fl-url", "main", "src/non_wasm")
        );

        // A branch and no folder — the repository root on that branch.
        assert_eq!(
            parse_repo_url("https://github.com/MyJetTools/fl-url/tree/dev").unwrap(),
            parsed("MyJetTools", "fl-url", "dev", "")
        );
    }

    /// A page ABOUT the repository is not a place in it, and neither is a query string.
    #[test]
    fn decoration_on_the_address_is_ignored() {
        assert_eq!(
            parse_repo_url("https://github.com/MyJetTools/fl-url/issues").unwrap(),
            parsed("MyJetTools", "fl-url", "", "")
        );

        assert_eq!(
            parse_repo_url("https://github.com/MyJetTools/fl-url?tab=readme#install").unwrap(),
            parsed("MyJetTools", "fl-url", "", "")
        );
    }

    #[test]
    fn what_is_not_a_repository_is_refused_with_the_reason() {
        assert!(parse_repo_url("").is_err());
        assert!(parse_repo_url("MyJetTools").is_err());

        // Named by host, because "could not parse" would send somebody looking for a typo they did not
        // make.
        let err = parse_repo_url("https://gitlab.com/owner/repo").unwrap_err();
        assert!(err.contains("gitlab.com"), "{err}");
    }

    #[test]
    fn a_folder_in_the_repository_is_normalised_and_may_be_empty() {
        assert_eq!(normalise_repo_path("  "), Ok(String::new()));
        assert_eq!(normalise_repo_path("/docs/"), Ok("docs".to_string()));
        assert_eq!(
            normalise_repo_path("docs\\design"),
            Ok("docs/design".to_string())
        );

        assert!(normalise_repo_path("../etc").is_err());
    }

    #[test]
    fn a_branch_is_a_path_segment_and_nothing_cleverer() {
        assert_eq!(normalise_branch(" main "), Ok("main".to_string()));
        assert_eq!(normalise_branch(""), Ok(String::new()));
        // A branch with a slash is a real branch and stays one.
        assert_eq!(
            normalise_branch("feature/x"),
            Ok("feature/x".to_string())
        );

        assert!(normalise_branch("a b").is_err());
        assert!(normalise_branch("a..b").is_err());
    }
}
