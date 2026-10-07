use rust_extensions::date_time::DateTimeAsMicroseconds;

use crate::board::GhActionModel;

/// The longest a build's name may be.
///
/// A cap rather than a truncation, so a caller is told rather than quietly shortened. It is generous: the
/// name of a build is a service and a version — `my-service v1.2.3` — and anything approaching this is a
/// sentence, which belongs on the thread.
pub const MAX_GH_ACTION_TITLE_LEN: usize = 120;

/// A build link as a caller hands it over.
///
/// `title` is optional, and that is deliberate: the useful half of a build reference is the url, and making
/// a name mandatory would only produce names copied out of the url by whoever was in a hurry. One is worked
/// out from the url instead — see [`title_from_url`].
pub struct NewGhAction {
    pub url: String,
    pub title: Option<String>,
}

/// What a caller wants to change about the builds a task points at.
///
/// Add and remove rather than "here is the new list", the same shape labels and document references use, and
/// for the same reason: a task collects builds one at a time over the life of the work, and a whole-list
/// write would drop whatever the caller had not read.
#[derive(Default)]
pub struct GhActionsPatch {
    pub add: Vec<NewGhAction>,
    pub remove: Vec<String>,
}

impl GhActionsPatch {
    pub fn is_empty(&self) -> bool {
        self.add.is_empty() && self.remove.is_empty()
    }

    /// Apply the patch to one build list, or refuse it entirely.
    ///
    /// **Adding a url that is already there is not a duplicate and not an error.** It is the same run, and
    /// the same build re-reported is what a re-run of a workflow step looks like from here. The existing
    /// entry keeps its moment — that is when this build was recorded against the work, and re-stamping it
    /// would rewrite history — and takes the new title if one was given, since a caller bothering to name it
    /// the second time is correcting the first.
    ///
    /// Removal after addition, so a url passed to both ends up removed — the order labels and documents use.
    /// A url that is not there is NOT an error: the caller's intent is already true.
    ///
    /// `now` is passed in rather than read here so the ordering is testable, and because every field of one
    /// update should be stamped with the same moment.
    pub fn apply(
        &self,
        builds: &mut Vec<GhActionModel>,
        now: DateTimeAsMicroseconds,
    ) -> Result<(), String> {
        for new_build in &self.add {
            let url = validate_url(&new_build.url)?;
            let title = validate_title(new_build.title.as_deref(), &url)?;

            match builds.iter_mut().find(|itm| itm.url == url) {
                Some(existing) => {
                    // Only when the caller actually named it. A second report with no title must not
                    // overwrite a good name with one derived from the url.
                    if has_title(new_build.title.as_deref()) {
                        existing.title = title;
                    }
                }
                None => builds.push(GhActionModel {
                    url,
                    title,
                    moment: now,
                }),
            }
        }

        for url in &self.remove {
            let url = url.trim();
            builds.retain(|itm| itm.url != url);
        }

        Ok(())
    }
}

/// Whether the caller said anything for a title.
fn has_title(src: Option<&str>) -> bool {
    src.map(str::trim).is_some_and(|itm| !itm.is_empty())
}

/// The url to store, or a refusal.
///
/// **Not checked against `github.com`**, deliberately, even though the field is named after it: an
/// enterprise install answers on its own domain, and refusing a link because of its host would refuse a real
/// build for a cosmetic reason. What IS checked is that this is a link at all — a run number or a branch
/// name stored here would draw a row nobody can click.
fn validate_url(src: &str) -> Result<String, String> {
    let url = src.trim();

    if url.is_empty() {
        return Err("a build needs a url — the link to its run".to_string());
    }

    let rest = url
        .strip_prefix("https://")
        .or_else(|| url.strip_prefix("http://"));

    match rest {
        Some(rest) if !rest.trim().is_empty() => Ok(url.to_string()),
        _ => Err(format!(
            "'{url}' is not a link — a build is named by the url of its run, like https://github.com/<owner>/<repo>/actions/runs/<id>"
        )),
    }
}

/// The name to store: what the caller said, else one worked out from the url.
fn validate_title(src: Option<&str>, url: &str) -> Result<String, String> {
    let Some(title) = src.map(str::trim).filter(|itm| !itm.is_empty()) else {
        return Ok(title_from_url(url));
    };

    let length = title.chars().count();

    if length > MAX_GH_ACTION_TITLE_LEN {
        return Err(format!(
            "that build's name is {length} characters — the limit is {MAX_GH_ACTION_TITLE_LEN}. A name is what the card draws; anything longer belongs on the thread"
        ));
    }

    Ok(title.to_string())
}

/// A name for a build nobody named, worked out from its url.
///
/// A run url reads `…/<owner>/<repo>/actions/runs/<id>`, and `repo #<id>` is what a person would have typed
/// anyway. Anything else falls back to the host and the last segment, which is still something a reader can
/// scan and is never empty — a blank name would draw a row with nothing in it.
fn title_from_url(url: &str) -> String {
    let without_scheme = url.split_once("://").map(|(_, rest)| rest).unwrap_or(url);

    let (host, path) = match without_scheme.split_once('/') {
        Some((host, path)) => (host, path),
        None => (without_scheme, ""),
    };

    let segments: Vec<&str> = path
        .split(['?', '#'])
        .next()
        .unwrap_or_default()
        .split('/')
        .filter(|itm| !itm.is_empty())
        .collect();

    // `runs` with the repo two segments before it and the run number after: that is a run url, and nothing
    // else on GitHub is shaped like it.
    if let Some(position) = segments.iter().position(|itm| *itm == "runs") {
        if let (Some(repo), Some(number)) = (
            position.checked_sub(2).and_then(|idx| segments.get(idx)),
            segments.get(position + 1),
        ) {
            return format!("{repo} #{number}");
        }
    }

    match (host.is_empty(), segments.last()) {
        (false, Some(last)) => format!("{host}/{last}"),
        (false, None) => host.to_string(),
        // Nothing recognisable to shorten to. The url itself is a worse name than any of the above and a
        // better one than nothing at all.
        (true, _) => url.trim().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RUN: &str = "https://github.com/my-org/my-service/actions/runs/18423";

    fn build(url: &str, title: Option<&str>) -> NewGhAction {
        NewGhAction {
            url: url.to_string(),
            title: title.map(|itm| itm.to_string()),
        }
    }

    fn now(seconds: i64) -> DateTimeAsMicroseconds {
        DateTimeAsMicroseconds::new(seconds * 1_000_000)
    }

    #[test]
    fn a_build_is_added_with_the_moment_it_was_attached() {
        let mut builds = Vec::new();

        GhActionsPatch {
            add: vec![build(RUN, Some("my-service v1.2.3"))],
            ..Default::default()
        }
        .apply(&mut builds, now(100))
        .unwrap();

        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].url, RUN);
        assert_eq!(builds[0].title, "my-service v1.2.3");
        assert_eq!(builds[0].moment, now(100));
    }

    /// The order they arrived in is the order the reader sees, and it is chronological because the moment is
    /// stamped as each one is attached.
    #[test]
    fn builds_keep_the_order_they_arrived_in() {
        let mut builds = Vec::new();

        GhActionsPatch {
            add: vec![build(RUN, Some("first"))],
            ..Default::default()
        }
        .apply(&mut builds, now(100))
        .unwrap();

        GhActionsPatch {
            add: vec![build("https://github.com/o/r/actions/runs/2", Some("second"))],
            ..Default::default()
        }
        .apply(&mut builds, now(200))
        .unwrap();

        let titles: Vec<&str> = builds.iter().map(|itm| itm.title.as_str()).collect();
        assert_eq!(titles, vec!["first", "second"]);
    }

    /// The same run reported twice is one build. The moment must not move — it is when this build was
    /// recorded against the work — and a name given the second time corrects the first.
    #[test]
    fn the_same_url_twice_is_one_build() {
        let mut builds = Vec::new();

        GhActionsPatch {
            add: vec![build(RUN, Some("first name"))],
            ..Default::default()
        }
        .apply(&mut builds, now(100))
        .unwrap();

        GhActionsPatch {
            add: vec![build(RUN, Some("corrected"))],
            ..Default::default()
        }
        .apply(&mut builds, now(500))
        .unwrap();

        assert_eq!(builds.len(), 1);
        assert_eq!(builds[0].title, "corrected");
        assert_eq!(builds[0].moment, now(100), "the first sighting is the one");
    }

    /// A re-report with no name must not overwrite a good name with one derived from the url.
    #[test]
    fn a_nameless_re_report_keeps_the_name_it_had() {
        let mut builds = Vec::new();

        GhActionsPatch {
            add: vec![build(RUN, Some("my-service v1.2.3"))],
            ..Default::default()
        }
        .apply(&mut builds, now(100))
        .unwrap();

        GhActionsPatch {
            add: vec![build(RUN, None)],
            ..Default::default()
        }
        .apply(&mut builds, now(200))
        .unwrap();

        assert_eq!(builds[0].title, "my-service v1.2.3");
    }

    #[test]
    fn removing_names_the_url_and_an_absent_one_is_not_an_error() {
        let mut builds = Vec::new();

        GhActionsPatch {
            add: vec![build(RUN, None)],
            ..Default::default()
        }
        .apply(&mut builds, now(100))
        .unwrap();

        GhActionsPatch {
            remove: vec![
                format!("  {RUN} "),
                "https://github.com/o/r/actions/runs/999".to_string(),
            ],
            ..Default::default()
        }
        .apply(&mut builds, now(200))
        .unwrap();

        assert!(builds.is_empty());
    }

    /// It has to be one order or the other, and removal last is the order every other list here uses.
    #[test]
    fn a_url_passed_to_both_ends_up_removed() {
        let mut builds = Vec::new();

        GhActionsPatch {
            add: vec![build(RUN, None)],
            remove: vec![RUN.to_string()],
        }
        .apply(&mut builds, now(100))
        .unwrap();

        assert!(builds.is_empty());
    }

    /// Something that is not a link would draw a row nobody can click, so it is refused rather than stored.
    #[test]
    fn a_url_that_is_not_a_link_is_refused() {
        for url in ["", "   ", "18423", "github.com/o/r", "ftp://x/y", "https://"] {
            let mut builds = Vec::new();

            let result = GhActionsPatch {
                add: vec![build(url, None)],
                ..Default::default()
            }
            .apply(&mut builds, now(100));

            assert!(result.is_err(), "'{url}' should not be accepted");
            assert!(builds.is_empty(), "a refused patch must change nothing");
        }
    }

    /// An enterprise install is not github.com, and refusing its builds would be a cosmetic rule with a real
    /// cost.
    #[test]
    fn a_host_that_is_not_github_com_is_accepted() {
        let mut builds = Vec::new();

        GhActionsPatch {
            add: vec![build("https://git.internal/o/r/actions/runs/7", None)],
            ..Default::default()
        }
        .apply(&mut builds, now(100))
        .unwrap();

        assert_eq!(builds.len(), 1);
    }

    #[test]
    fn a_name_nobody_wrote_is_worked_out_from_the_url() {
        assert_eq!(title_from_url(RUN), "my-service #18423");
        assert_eq!(
            title_from_url("https://git.internal/o/repo/actions/runs/7?check_suite_focus=true"),
            "repo #7"
        );

        // Not a run url — the host and the last segment still read better than the whole thing.
        assert_eq!(
            title_from_url("https://github.com/my-org/my-service/releases/tag/v1.2.3"),
            "github.com/v1.2.3"
        );
        assert_eq!(title_from_url("https://github.com"), "github.com");
    }

    /// A derived name is only a fallback: what the caller wrote wins, trimmed.
    #[test]
    fn a_written_name_wins_over_a_derived_one() {
        assert_eq!(
            validate_title(Some("  my-service v1.2.3  "), RUN).unwrap(),
            "my-service v1.2.3"
        );
        assert_eq!(validate_title(Some("   "), RUN).unwrap(), "my-service #18423");
        assert_eq!(validate_title(None, RUN).unwrap(), "my-service #18423");
    }

    #[test]
    fn an_oversized_name_is_refused() {
        let long = "a".repeat(MAX_GH_ACTION_TITLE_LEN + 1);
        assert!(validate_title(Some(&long), RUN).is_err());

        let just_fits = "a".repeat(MAX_GH_ACTION_TITLE_LEN);
        assert!(validate_title(Some(&just_fits), RUN).is_ok());
    }

    #[test]
    fn an_empty_patch_is_recognised() {
        assert!(GhActionsPatch::default().is_empty());

        assert!(
            !GhActionsPatch {
                add: vec![build(RUN, None)],
                ..Default::default()
            }
            .is_empty()
        );

        assert!(
            !GhActionsPatch {
                remove: vec![RUN.to_string()],
                ..Default::default()
            }
            .is_empty()
        );
    }
}
