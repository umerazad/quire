//! Personal Git: commits a notebook folder, brings in what the remote has, and pushes without
//! force, as the reference app's `git-sync.ts` does. Git runs as a separate program with the
//! user's own configuration, so credentials come from Git's credential helpers and SSH agent;
//! prompts are disabled, following Zed's rules for background Git, so a missing credential fails
//! instead of waiting for input nobody can give.

use crate::provider;
use anyhow::{Context as _, Result, anyhow, bail};
use std::{
    ffi::{OsStr, OsString},
    fs,
    io::{self, Read as _},
    path::{Path, PathBuf},
    process::{Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};

/// Folders holding live databases and private AI transcripts. They stay on this computer: the
/// repository's local exclude file lists them, never a tracked `.gitignore`.
pub const LIVE_PATHS: [&str; 2] = [".quire/devices", ".quire/ai"];

const RETAINED: &str = "Local notes are retained.";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Profile {
    Current,
    Work,
    Personal,
}

/// How to run Git. The default runs `git` from `PATH` with the user's configuration; tests add
/// environment entries that isolate Git from the user's own settings.
#[derive(Clone, Debug)]
pub struct Git {
    program: PathBuf,
    env: Vec<(OsString, OsString)>,
    timeout: Duration,
}

impl Default for Git {
    fn default() -> Self {
        Self {
            program: "git".into(),
            env: Vec::new(),
            timeout: Duration::from_secs(60),
        }
    }
}

impl Git {
    pub fn new(program: impl Into<PathBuf>) -> Self {
        Self {
            program: program.into(),
            ..Self::default()
        }
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.env.push((key.into(), value.into()));
        self
    }

    pub fn timeout(mut self, timeout: Duration) -> Self {
        self.timeout = timeout;
        self
    }

    fn inherited(&self, key: &str) -> Option<OsString> {
        self.env
            .iter()
            .rev()
            .find(|(name, _)| name == key)
            .map(|(_, value)| value.clone())
            .or_else(|| std::env::var_os(key))
    }

    /// A command acting on the repository at `root` only: inherited repository overrides are
    /// removed, file system monitors are off, and nothing can prompt.
    fn command(&self, root: &Path, args: &[&OsStr], ssh: Option<&str>) -> Command {
        let mut command = Command::new(&self.program);
        command
            .current_dir(root)
            .args(["-c", "core.fsmonitor=false"])
            .args(["-c", "credential.interactive=false"])
            .args(["-c", "core.askPass="])
            .arg("--no-pager")
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());
        for key in [
            "GIT_DIR",
            "GIT_WORK_TREE",
            "GIT_INDEX_FILE",
            "GIT_COMMON_DIR",
            "GIT_OBJECT_DIRECTORY",
            "GIT_ALTERNATE_OBJECT_DIRECTORIES",
            "GIT_ASKPASS",
            "SSH_ASKPASS",
        ] {
            command.env_remove(key);
        }
        command.envs(self.env.iter().map(|(key, value)| (key, value)));
        command.env("GIT_TERMINAL_PROMPT", "0");
        if let Some(ssh) = ssh {
            command.env("GIT_SSH_COMMAND", ssh);
        }
        command
    }

    fn output(&self, root: &Path, args: &[&OsStr], ssh: Option<&str>) -> Result<Output> {
        let mut child = self.command(root, args, ssh).spawn().map_err(|error| {
            if error.kind() == io::ErrorKind::NotFound {
                anyhow!("Git is unavailable. Install Git on this computer and retry.")
            } else {
                anyhow!("Cannot start Git: {error}. Check the Git installation and retry.")
            }
        })?;
        let read = |pipe: Option<Box<dyn io::Read + Send>>| {
            std::thread::spawn(move || {
                let mut bytes = Vec::new();
                if let Some(mut pipe) = pipe {
                    pipe.read_to_end(&mut bytes).ok();
                }
                String::from_utf8_lossy(&bytes).trim().to_owned()
            })
        };
        let stdout = read(child.stdout.take().map(|pipe| Box::new(pipe) as _));
        let stderr = read(child.stderr.take().map(|pipe| Box::new(pipe) as _));
        let deadline = Instant::now() + self.timeout;
        let status: ExitStatus = loop {
            if let Some(status) = child.try_wait()? {
                break status;
            }
            if Instant::now() >= deadline {
                child.kill().ok();
                child.wait().ok();
                bail!(
                    "Git did not finish. Check the network and Git credentials, then retry. {RETAINED}"
                );
            }
            std::thread::sleep(Duration::from_millis(2));
        };
        Ok(Output {
            code: status.code().unwrap_or(-1),
            stdout: stdout.join().unwrap_or_default(),
            stderr: stderr.join().unwrap_or_default(),
        })
    }
}

/// The SSH command for fetch and push: the one Git would use anyway, in batch mode, so a missing
/// key or unknown host fails rather than asking. A custom `GIT_SSH` program is left alone.
pub fn batch_ssh_command(
    ssh_command_env: Option<&str>,
    core_ssh_command: Option<&str>,
    git_ssh_env: Option<&str>,
) -> Option<String> {
    let configured = ssh_command_env
        .or(core_ssh_command)
        .map(str::trim)
        .filter(|command| !command.is_empty());
    let base = match configured {
        Some(configured) => configured,
        None if git_ssh_env.is_some_and(|program| !program.trim().is_empty()) => return None,
        None => "ssh",
    };
    Some(format!("{base} -o BatchMode=yes -o ConnectTimeout=15"))
}

struct Output {
    code: i32,
    stdout: String,
    stderr: String,
}

/// Validates a remote typed in settings: Personal only, and never one carrying credentials, which
/// belong in Git's credential helper. An empty remote turns Git sync off.
pub fn check_remote(profile: Profile, remote: &str) -> Result<Option<String>> {
    if profile != Profile::Personal {
        bail!("Git sync is available only in Personal. Select the Personal profile first.");
    }
    let remote = remote.trim();
    if remote.is_empty() {
        return Ok(None);
    }
    if remote.starts_with('-') || remote.contains(['\r', '\n', '\0']) || carries_user(remote) {
        bail!(
            "The Git remote is invalid or contains credentials. Use a remote URL with credentials managed by Git."
        );
    }
    Ok(Some(remote.to_owned()))
}

fn carries_user(remote: &str) -> bool {
    let lower = remote.to_ascii_lowercase();
    ["http://", "https://"]
        .iter()
        .find_map(|scheme| lower.strip_prefix(scheme))
        .is_some_and(|rest| {
            rest.split('/')
                .next()
                .is_some_and(|host| host.contains('@'))
        })
}

fn supported_remote(remote: &str) -> bool {
    let lower = remote.to_ascii_lowercase();
    let scheme = lower.split_once("://").map(|(scheme, _)| scheme);
    !(remote.starts_with('-')
        || remote.contains(['\r', '\n', '\0'])
        || remote.contains("::")
        || carries_user(remote)
        || scheme.is_some_and(|scheme| {
            scheme.bytes().all(|byte| byte.is_ascii_alphabetic())
                && !matches!(scheme, "https" | "ssh" | "file")
        }))
}

/// What Personal Git syncs: the notebook folder of a profile and its configured remote.
#[derive(Clone, Copy, Debug)]
pub struct Target<'a> {
    pub profile: Profile,
    pub root: &'a Path,
    pub remote: Option<&'a str>,
}

/// A push the remote accepted: the commit now at the remote branch, and when.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Pushed {
    pub commit: String,
    pub at: u64,
}

struct Repository<'a> {
    git: &'a Git,
    root: PathBuf,
    branch: String,
    ssh: Option<String>,
}

impl Repository<'_> {
    fn execute(&self, args: &[&str]) -> Result<Output> {
        let ssh = match args.first() {
            Some(&"fetch" | &"push") => self.ssh.as_deref(),
            _ => None,
        };
        let args: Vec<&OsStr> = args.iter().map(OsStr::new).collect();
        self.git.output(&self.root, &args, ssh)
    }

    fn run(&self, args: &[&str]) -> Result<String> {
        let output = self.execute(args)?;
        if output.code != 0 {
            let detail = if output.stderr.is_empty() {
                &output.stdout
            } else {
                &output.stderr
            };
            bail!(
                "Git {} failed: {}. Check the repository, Git identity and credentials, then retry. {RETAINED}",
                args[0],
                truncate(detail, 1500)
            );
        }
        Ok(output.stdout)
    }

    fn succeeds(&self, args: &[&str]) -> Result<bool> {
        Ok(self.execute(args)?.code == 0)
    }

    fn head(&self, reference: &str) -> Result<Option<String>> {
        let output = self.execute(&["rev-parse", "--verify", "--quiet", reference])?;
        Ok((output.code == 0).then_some(output.stdout))
    }

    /// Commits every file except those `excluded` (Git pathspecs).
    fn commit_all(&self, message: &str, excluded: &[&str]) -> Result<()> {
        let mut add = vec!["add", "-A", "--", "."];
        add.extend_from_slice(excluded);
        self.run(&add)?;
        match self.execute(&["diff", "--cached", "--quiet"])?.code {
            0 => Ok(()),
            1 => self.run(&["commit", "-m", message]).map(drop),
            _ => bail!("Git could not inspect staged changes. Repair the repository and retry."),
        }
    }

    fn push(&self, at: u64) -> Result<Pushed> {
        let destination = format!("HEAD:refs/heads/{}", self.branch);
        self.run(&["push", "--porcelain", "origin", &destination])?;
        let commit = self
            .head("HEAD")?
            .context("The pushed commit disappeared. Repair the repository and retry.")?;
        Ok(Pushed { commit, at })
    }

    fn remote_ref(&self) -> String {
        format!("refs/remotes/origin/{}", self.branch)
    }
}

fn truncate(text: &str, limit: usize) -> &str {
    text.char_indices()
        .nth(limit)
        .map_or(text, |(index, _)| &text[..index])
}

fn real_root(root: &Path) -> Result<PathBuf> {
    fs::canonicalize(root).with_context(|| {
        format!(
            "The notebook folder {} is unavailable. Reconnect it or choose its actual folder again before syncing.",
            root.display()
        )
    })
}

fn is_top_level(git: &Git, root: &Path) -> Result<Option<bool>> {
    let top = git.output(
        root,
        &[OsStr::new("rev-parse"), OsStr::new("--show-toplevel")],
        None,
    )?;
    if top.code != 0 {
        return Ok(None);
    }
    Ok(Some(
        fs::canonicalize(&top.stdout).is_ok_and(|top| top == root),
    ))
}

fn exclude_live_data(repository: &Repository) -> Result<()> {
    let name = repository
        .execute(&["rev-parse", "--git-path", "info/exclude"])
        .ok()
        .filter(|output| output.code == 0)
        .context(
            "Git could not locate the repository's exclude file. Repair the repository and retry.",
        )?;
    let file = repository.root.join(name.stdout);
    let existing = match fs::read_to_string(&file) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => String::new(),
        Err(error) => {
            return Err(error).context(
                "Cannot read the repository's exclude file. Check its permissions and retry.",
            );
        }
    };
    let present: Vec<&str> = existing.lines().collect();
    let missing: Vec<String> = LIVE_PATHS
        .iter()
        .map(|path| format!("/{path}/"))
        .filter(|line| !present.contains(&line.as_str()))
        .collect();
    if missing.is_empty() {
        return Ok(());
    }
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let mut text = existing.clone();
    if !text.is_empty() && !text.ends_with('\n') {
        text.push('\n');
    }
    for line in missing {
        text.push_str(&line);
        text.push('\n');
    }
    fs::write(&file, text)
        .context("Cannot update the repository's exclude file. Check its permissions and retry.")
}

/// On opening a notebook that is the root of a Git repository, keeps live databases and AI
/// transcripts out of commits. Returns warnings; Git problems never stop the notebook opening.
pub fn protect_notebook(git: &Git, root: &Path) -> Vec<String> {
    let protect = || -> Result<Vec<String>> {
        let root = real_root(root)?;
        if is_top_level(git, &root)? != Some(true) {
            return Ok(Vec::new());
        }
        let repository = Repository {
            git,
            root,
            branch: String::new(),
            ssh: None,
        };
        exclude_live_data(&repository)?;
        let mut args = vec!["ls-files", "--"];
        args.extend(LIVE_PATHS);
        let tracked = repository.execute(&args)?;
        Ok(if tracked.stdout.is_empty() {
            Vec::new()
        } else {
            vec!["Quire's live databases under .quire are tracked by Git, so commits can capture them mid-write. In the notebook folder, run git rm -r --cached .quire/devices .quire/ai and commit.".into()]
        })
    };
    protect().unwrap_or_default()
}

/// Checks the Personal repository and its remote, creating the repository on first use.
fn open<'a>(git: &'a Git, target: &Target) -> Result<Repository<'a>> {
    if target.profile != Profile::Personal {
        bail!("Git sync is available only for Personal. Select the Personal profile.");
    }
    let remote = target
        .remote
        .context("Choose a Personal notebook and configure its Git remote before syncing.")?;
    if !supported_remote(remote) {
        bail!("Unsupported Git remote. Use an HTTPS, SSH, or local repository URL.");
    }
    let root = real_root(target.root)?;
    let mut repository = Repository {
        git,
        root,
        branch: String::new(),
        ssh: None,
    };
    match is_top_level(git, &repository.root)? {
        None if fs::symlink_metadata(repository.root.join(".git")).is_ok() => bail!(
            "The Personal Git repository is unavailable. Repair its .git metadata before retrying."
        ),
        None => {
            repository.run(&["init", "-b", "main"])?;
        }
        Some(false) => bail!(
            "The notebook is inside a larger Git repository. Choose the repository root or a separate Personal folder."
        ),
        Some(true) => {}
    }
    repository.branch = repository.run(&["symbolic-ref", "--short", "HEAD"])?;
    let mut tracked = vec!["ls-files", "--"];
    tracked.extend(LIVE_PATHS);
    if !repository.run(&tracked)?.is_empty() {
        bail!(
            "Live device databases are tracked by Git. Back up the notebook, untrack .quire/devices and .quire/ai, and retry. Portable snapshots under .quire/history remain available for Git sync."
        );
    }
    exclude_live_data(&repository)?;
    let branch = repository.branch.clone();
    repository.run(&["check-ref-format", "--branch", &branch])?;
    let origin = repository.execute(&["remote", "get-url", "origin"])?;
    if origin.code != 0 {
        repository.run(&["remote", "add", "origin", remote])?;
    } else if origin.stdout != remote {
        bail!(
            "The repository's origin differs from the configured remote. Enter its existing origin URL in Settings or change it using Git."
        );
    }
    if !repository
        .run(&["diff", "--name-only", "--diff-filter=U"])?
        .is_empty()
    {
        bail!(
            "The Personal repository has unresolved conflicts. Resolve them in Git before syncing."
        );
    }
    refuse_placeholders(&repository.root)?;
    let core_ssh = repository
        .execute(&["config", "--get", "core.sshCommand"])
        .ok()
        .filter(|output| output.code == 0)
        .map(|output| output.stdout);
    let inherited = |key| {
        git.inherited(key)
            .map(|value| value.to_string_lossy().into_owned())
    };
    repository.ssh = batch_ssh_command(
        inherited("GIT_SSH_COMMAND").as_deref(),
        core_ssh.as_deref(),
        inherited("GIT_SSH").as_deref(),
    );
    Ok(repository)
}

/// A file a provider keeps only in the cloud would be committed as deleted or fail to read, so
/// syncing waits until the provider has downloaded the whole notebook.
fn refuse_placeholders(root: &Path) -> Result<()> {
    let found = provider::scan(root)?;
    let Some(first) = found.placeholders.first() else {
        return Ok(());
    };
    let provider = provider::detect(root);
    let place = provider.map_or("the cloud", provider::Provider::name);
    let fix = provider.map_or(
        "Make the notebook folder available offline",
        provider::Provider::keep_offline,
    );
    bail!(
        "{} notes, including {}, are only in {place} and not on this computer, so a commit would record them as deleted. {fix}, wait for the download to finish, then sync again. {RETAINED}",
        found.placeholders.len(),
        first.display()
    )
}

fn iso(at: u64) -> String {
    let stamp = crate::backup::stamp(at);
    let (date, time) = stamp.split_once('T').unwrap_or((&stamp, ""));
    let parts: Vec<&str> = time.trim_end_matches('Z').split('-').collect();
    match parts[..] {
        [hour, minute, second, millis] => format!("{date}T{hour}:{minute}:{second}.{millis}Z"),
        _ => stamp,
    }
}

/// Syncs a Quire notebook folder. Each computer adds only its own immutable change files, so two
/// computers' commits usually merge cleanly: Quire commits first, merges whatever the remote has,
/// then pushes. A merge that does conflict, because someone edited a synced file outside Quire
/// or two computers share one identity, is aborted and reported, never resolved by Quire.
pub fn sync_quire(git: &Git, target: &Target, now: u64) -> Result<Pushed> {
    let repository = open(git, target)?;
    // A writer may be publishing meanwhile; its temporary files start with a dot and become
    // whole change files only by a rename, so leaving them out commits only complete files.
    repository.commit_all(
        &format!("Quire changes {}", iso(now)),
        &[":(exclude,glob)devices/**/.*"],
    )?;
    repository.run(&["fetch", "origin"])?;
    let remote_ref = repository.remote_ref();
    if repository.head(&remote_ref)?.is_some() {
        if repository.head("HEAD")?.is_none() {
            repository.run(&["merge", "--ff-only", &remote_ref])?;
        } else if repository
            .execute(&["merge", "--no-edit", &remote_ref])?
            .code
            != 0
        {
            repository.execute(&["merge", "--abort"])?;
            bail!(
                "The remote has changes to the same files as this computer, which Quire never makes. Someone edited the notebook folder outside Quire, or two computers share one device identity; reconcile it in Git and retry. {RETAINED}"
            );
        }
    }
    if repository.head("HEAD")?.is_none() {
        bail!("There is nothing to push yet. Save a page and sync again.");
    }
    repository.push(now)
}

/// Syncs a Markdown folder notebook. Quire never merges notes: it fast-forwards when only the
/// remote moved and the folder is clean, refuses when both sides changed, then commits and
/// pushes.
pub fn sync_folder(git: &Git, target: &Target, now: u64) -> Result<Pushed> {
    let repository = open(git, target)?;
    repository.run(&["fetch", "origin"])?;
    let head = repository.head("HEAD")?;
    let remote_ref = repository.remote_ref();
    if let Some(remote_head) = repository.head(&remote_ref)? {
        refuse_remote_live_data(&repository, &remote_ref)?;
        let dirty = has_local_changes(&repository)?;
        match head {
            None if dirty => bail!(
                "The remote already contains notes and this folder has uncommitted files. Clone the remote into a new folder, or reconcile the files using Git, then retry."
            ),
            None => {
                repository.run(&["merge", "--ff-only", &remote_ref])?;
            }
            Some(head) if head != remote_head => {
                let behind =
                    repository.succeeds(&["merge-base", "--is-ancestor", "HEAD", &remote_ref])?;
                let ahead =
                    repository.succeeds(&["merge-base", "--is-ancestor", &remote_ref, "HEAD"])?;
                if !behind && !ahead {
                    bail!(
                        "Local and remote Personal notes have diverged. Both versions are retained. Reconcile them in Git and retry; Quire will not merge automatically."
                    );
                }
                if behind {
                    if dirty {
                        bail!(
                            "Remote notes changed while this folder also has local edits. Your backup and local edits are retained. Reconcile the versions in Git before syncing."
                        );
                    }
                    repository.run(&["merge", "--ff-only", &remote_ref])?;
                }
            }
            Some(_) => {}
        }
    } else if !repository
        .run(&["for-each-ref", "--format=%(refname)", "refs/remotes/origin"])?
        .is_empty()
    {
        bail!(
            "The remote uses a different branch. Open a clone on the correct branch and retry syncing."
        );
    }
    repository.commit_all(&format!("Save personal notes {}", iso(now)), &[])?;
    if repository.head("HEAD")?.is_none() {
        bail!(
            "There are no tracked notes to push. Add a note or review .gitignore, then sync again."
        );
    }
    repository.push(now)
}

/// Uncommitted work, including ignored files a fast-forward could replace, except the live
/// folders the remote is known not to track.
fn has_local_changes(repository: &Repository) -> Result<bool> {
    let status = repository.run(&[
        "status",
        "--porcelain",
        "--untracked-files=all",
        "--ignored",
    ])?;
    Ok(status.lines().any(|line| {
        let path = line.get(3..).unwrap_or_default().trim_matches('"');
        !LIVE_PATHS
            .iter()
            .any(|live| path == *live || path.starts_with(&format!("{live}/")))
    }))
}

fn refuse_remote_live_data(repository: &Repository, remote_ref: &str) -> Result<()> {
    let mut args = vec!["ls-tree", "-r", "--name-only", remote_ref, "--"];
    args.extend(LIVE_PATHS);
    if !repository.run(&args)?.is_empty() {
        bail!(
            "The remote tracks live device databases under .quire, so bringing it in would replace this computer's. Untrack .quire/devices and .quire/ai in the remote using Git, then retry. {RETAINED}"
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn remotes_with_credentials_or_unsupported_transports_are_refused() {
        for remote in [
            "https://user:secret@example.com/notes.git",
            "http://token@example.com/n",
            "--upload-pack=x",
            "a\nb",
        ] {
            assert!(check_remote(Profile::Personal, remote).is_err(), "{remote}");
            assert!(!supported_remote(remote), "{remote}");
        }
        for remote in [
            "ext::sh -c x",
            "git://example.com/notes",
            "ftp://example.com/n",
        ] {
            assert!(!supported_remote(remote), "{remote}");
        }
        for remote in [
            "https://example.com/a@b/notes.git",
            "git@example.com:me/notes.git",
            "ssh://git@example.com/notes",
            "file:///tmp/notes.git",
            "/tmp/notes.git",
        ] {
            assert!(supported_remote(remote), "{remote}");
            assert_eq!(
                check_remote(Profile::Personal, &format!(" {remote} "))
                    .unwrap()
                    .as_deref(),
                Some(remote)
            );
        }
        assert_eq!(check_remote(Profile::Personal, "  ").unwrap(), None);
        for profile in [Profile::Current, Profile::Work] {
            let error = check_remote(profile, "https://example.com/n.git").unwrap_err();
            assert!(error.to_string().contains("only in Personal"));
        }
    }

    #[test]
    fn remote_commands_use_batch_ssh_and_never_prompt() {
        assert_eq!(
            batch_ssh_command(None, None, None).as_deref(),
            Some("ssh -o BatchMode=yes -o ConnectTimeout=15")
        );
        assert_eq!(
            batch_ssh_command(Some("ssh -i key"), Some("ignored"), None).as_deref(),
            Some("ssh -i key -o BatchMode=yes -o ConnectTimeout=15")
        );
        assert_eq!(
            batch_ssh_command(None, Some(" plink "), None).as_deref(),
            Some("plink -o BatchMode=yes -o ConnectTimeout=15")
        );
        assert_eq!(
            batch_ssh_command(None, None, Some("/usr/bin/custom-ssh")),
            None
        );

        let git = Git::default()
            .env("GIT_ASKPASS", "/bin/false")
            .env("GIT_TERMINAL_PROMPT", "1");
        let command = git.command(
            Path::new("."),
            &[OsStr::new("fetch")],
            Some("ssh -o BatchMode=yes"),
        );
        let envs: Vec<(String, Option<String>)> = command
            .get_envs()
            .map(|(key, value)| {
                (
                    key.to_string_lossy().into_owned(),
                    value.map(|value| value.to_string_lossy().into_owned()),
                )
            })
            .collect();
        let last = |key: &str| {
            envs.iter()
                .rev()
                .find(|(name, _)| name == key)
                .map(|(_, value)| value.clone())
        };
        assert_eq!(last("GIT_TERMINAL_PROMPT"), Some(Some("0".into())));
        assert_eq!(
            last("GIT_SSH_COMMAND"),
            Some(Some("ssh -o BatchMode=yes".into()))
        );
        assert_eq!(last("GIT_DIR"), Some(None));
        let args: Vec<String> = command
            .get_args()
            .map(|arg| arg.to_string_lossy().into_owned())
            .collect();
        assert!(args.contains(&"credential.interactive=false".to_owned()));
        assert!(
            !args.iter().any(|arg| arg.contains("credential.helper")),
            "helpers stay enabled: {args:?}"
        );
        assert!(
            !args
                .iter()
                .any(|arg| arg.starts_with("--force") || arg == "-f")
        );
    }

    #[test]
    fn commit_messages_carry_an_iso_time() {
        assert_eq!(iso(1_793_456_585_123), "2026-10-31T14:23:05.123Z");
    }

    #[test]
    fn a_missing_git_program_says_to_install_git() {
        let git = Git::new("/nonexistent/quire-test-git");
        let error = git
            .output(Path::new("."), &[OsStr::new("status")], None)
            .err()
            .unwrap();
        assert_eq!(
            error.to_string(),
            "Git is unavailable. Install Git on this computer and retry."
        );
    }
}
