//! Measures a synthetic year of Personal Git growth for both notebook kinds.
//!
//! `cargo run --release -p quire_store --example git_growth -- [days] [folder]`
//!
//! Two computers share one bare remote in a fresh folder under `folder` (default: the system
//! temporary folder). Every day the home computer writes a daily page and edits two project
//! pages; on weekdays the office computer also edits a project page and a meeting page. Each
//! computer syncs once at the end of its session, so on weekdays one sync brings in the other's
//! work. The Markdown notebook syncs through `sync_folder`; the Quire notebook saves every edit
//! as a revision, publishes once per session, and syncs through `sync_quire`. Git runs with a
//! private configuration that sets only an identity, so the user's settings and credentials are
//! never read.

use anyhow::{Context as _, Result, ensure};
use quire_store::{
    git_sync::{self, Git, Profile, Target},
    notebook::{Edit, NotebookStore, RescanLimits},
};
use std::{
    fs,
    path::{Path, PathBuf},
    process::Command,
    time::{Duration, Instant},
};

const DAY: u64 = 86_400_000;
const START: u64 = 1_767_225_600_000;

struct Random(u64);

impl Random {
    fn next(&mut self) -> u64 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        self.0 >> 33
    }

    fn line(&mut self) -> String {
        const WORDS: [&str; 16] = [
            "review", "draft", "#project", "call", "notes", "[[Plan]]", "ship", "fix", "read",
            "design", "budget", "#idea", "follow", "up", "meeting", "TODO",
        ];
        let words: Vec<&str> = (0..6 + self.next() % 10)
            .map(|_| WORDS[(self.next() % 16) as usize])
            .collect();
        format!("- {}\n", words.join(" "))
    }
}

/// One page edit: a page and the lines it gains.
struct Change {
    page: String,
    lines: Vec<String>,
}

fn session(random: &mut Random, day: u64, office: bool) -> Vec<Change> {
    let date = date_of(day);
    let mut changes = Vec::new();
    if office {
        changes.push(Change {
            page: format!("Meetings/{date}"),
            lines: (0..8).map(|_| random.line()).collect(),
        });
        changes.push(Change {
            page: format!("Projects/Project {}", random.next() % 12),
            lines: (0..3).map(|_| random.line()).collect(),
        });
    } else {
        changes.push(Change {
            page: format!("Daily/{date}"),
            lines: (0..12).map(|_| random.line()).collect(),
        });
        for _ in 0..2 {
            changes.push(Change {
                page: format!("Projects/Project {}", random.next() % 12),
                lines: (0..2).map(|_| random.line()).collect(),
            });
        }
    }
    changes
}

fn date_of(day: u64) -> String {
    let days = (START / DAY + day) as i64;
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let date = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + i64::from(month <= 2);
    format!("{year:04}-{month:02}-{date:02}")
}

struct Lab {
    directory: PathBuf,
    git: Git,
    config: PathBuf,
}

impl Lab {
    fn run(&self, root: &Path, args: &[&str]) -> Result<String> {
        let output = Command::new("git")
            .current_dir(root)
            .args(args)
            .env("GIT_CONFIG_GLOBAL", &self.config)
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .output()?;
        ensure!(
            output.status.success(),
            "git {args:?} failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
    }

    fn bare(&self, name: &str) -> Result<String> {
        let remote = self.directory.join(name).display().to_string();
        self.run(
            &self.directory,
            &["init", "-q", "--bare", "-b", "main", &remote],
        )?;
        Ok(remote)
    }
}

fn size(path: &Path) -> u64 {
    fs::read_dir(path)
        .map(|entries| {
            entries
                .flatten()
                .map(|entry| match entry.metadata() {
                    Ok(metadata) if metadata.is_dir() => size(&entry.path()),
                    Ok(metadata) => metadata.len(),
                    Err(_) => 0,
                })
                .sum()
        })
        .unwrap_or(0)
}

fn percentile(sorted: &[Duration], fraction: f64) -> f64 {
    let index = ((sorted.len() as f64 - 1.0) * fraction).round() as usize;
    sorted[index].as_secs_f64() * 1000.0
}

struct Measured {
    syncs: Vec<Duration>,
    publishes: Vec<Duration>,
}

fn report(lab: &Lab, kind: &str, remote: &str, worktree: &Path, measured: Measured) -> Result<()> {
    let remote = Path::new(remote);
    let commits = lab.run(remote, &["rev-list", "--count", "main"])?;
    let files = lab
        .run(remote, &["ls-tree", "-r", "--name-only", "main"])?
        .lines()
        .count();
    let loose = size(remote);
    lab.run(remote, &["gc", "-q"])?;
    let packed = size(remote);
    let notes = size(worktree) - size(&worktree.join(".git"));
    let mut syncs = measured.syncs;
    let quarters: Vec<String> = syncs
        .chunks(syncs.len().div_ceil(4))
        .map(|chunk| {
            let mut chunk = chunk.to_vec();
            chunk.sort();
            format!("{:.0}", percentile(&chunk, 0.5))
        })
        .collect();
    syncs.sort();
    println!("{kind}:");
    println!(
        "  remote commits {commits}, tracked files {files}, working files {:.1} KiB",
        notes as f64 / 1024.0
    );
    println!(
        "  remote repository {:.1} KiB as pushed, {:.1} KiB after git gc",
        loose as f64 / 1024.0,
        packed as f64 / 1024.0
    );
    println!(
        "  {} syncs: median {:.0} ms, p95 {:.0} ms, max {:.0} ms; median by quarter {} ms",
        syncs.len(),
        percentile(&syncs, 0.5),
        percentile(&syncs, 0.95),
        percentile(&syncs, 1.0),
        quarters.join(" / ")
    );
    if !measured.publishes.is_empty() {
        let mut publishes = measured.publishes;
        publishes.sort();
        println!(
            "  {} publications: median {:.1} ms, max {:.1} ms",
            publishes.len(),
            percentile(&publishes, 0.5),
            percentile(&publishes, 1.0)
        );
    }
    Ok(())
}

fn text_with(current: &str, change: &Change) -> String {
    let mut text = if current.is_empty() {
        format!("# {}\n", change.page)
    } else {
        current.to_owned()
    };
    for line in &change.lines {
        text.push_str(line);
    }
    text
}

fn folder_year(lab: &Lab, days: u64) -> Result<()> {
    let remote = lab.bare("folder.git")?;
    let computers: Vec<PathBuf> = ["folder-home", "folder-office"]
        .iter()
        .map(|name| lab.directory.join(name))
        .collect();
    for root in &computers {
        fs::create_dir(root)?;
    }
    let mut random = Random(7);
    let mut syncs = Vec::new();
    for day in 0..days {
        let weekday = (day + 4) % 7 < 5;
        for (index, root) in computers.iter().enumerate() {
            let office = index == 1;
            if office && !weekday {
                continue;
            }
            let target = Target {
                profile: Profile::Personal,
                root,
                remote: Some(&remote),
            };
            // Bring in the other computer's work before editing, as a person opening the
            // notebook would.
            if day > 0 || office {
                git_sync::sync_folder(&lab.git, &target, START + day * DAY)?;
            }
            for change in session(&mut random, day, office) {
                let path = root.join(format!("{}.md", change.page));
                fs::create_dir_all(path.parent().context("page folder")?)?;
                let current = fs::read_to_string(&path).unwrap_or_default();
                fs::write(&path, text_with(&current, &change))?;
            }
            let started = Instant::now();
            git_sync::sync_folder(&lab.git, &target, START + day * DAY + 1)?;
            syncs.push(started.elapsed());
        }
    }
    report(
        lab,
        "Markdown folder notebook",
        &remote,
        &computers[0],
        Measured {
            syncs,
            publishes: Vec::new(),
        },
    )
}

fn quire_year(lab: &Lab, days: u64) -> Result<()> {
    let remote = lab.bare("quire.git")?;
    let mut computers = Vec::new();
    for name in ["home", "office"] {
        let root = lab.directory.join(format!("quire-{name}"));
        fs::create_dir(&root)?;
        let store = NotebookStore::open(&lab.directory.join(format!("{name}.sqlite")), "nb", name)?;
        computers.push((root, store));
    }
    fs::write(
        computers[0].0.join("quire-notebook.json"),
        br#"{"format":1,"id":"nb","created":1}"#,
    )?;
    let mut random = Random(7);
    let (mut syncs, mut publishes) = (Vec::new(), Vec::new());
    for day in 0..days {
        let weekday = (day + 4) % 7 < 5;
        for (index, (root, store)) in computers.iter().enumerate() {
            let office = index == 1;
            if office && !weekday {
                continue;
            }
            let target = Target {
                profile: Profile::Personal,
                root,
                remote: Some(&remote),
            };
            let now = START + day * DAY;
            if day > 0 || office {
                git_sync::sync_quire(&lab.git, &target, now)?;
                store.rescan(root, RescanLimits::default(), None)?;
            }
            for change in session(&mut random, day, office) {
                let page = change.page.clone();
                let heads = store.heads(&page)?;
                let current = match heads.first() {
                    Some(head) => store
                        .get(head)?
                        .and_then(|revision| {
                            revision.root().field("text").as_str().map(str::to_owned)
                        })
                        .unwrap_or_default(),
                    None => String::new(),
                };
                // Autosave records each line as its own revision.
                let mut text = current;
                let mut base = heads;
                for line in &change.lines {
                    text = text_with(
                        &text,
                        &Change {
                            page: page.clone(),
                            lines: vec![line.clone()],
                        },
                    );
                    let saved = store.save(&Edit {
                        page: page.clone(),
                        base,
                        path: format!("{page}.md"),
                        text: text.clone(),
                        deleted: false,
                        created: now,
                        recovery: None,
                    })?;
                    base = vec![saved.revision_id().to_owned()];
                }
            }
            let started = Instant::now();
            store.publish_to(root, now + 1)?;
            publishes.push(started.elapsed());
            let started = Instant::now();
            git_sync::sync_quire(&lab.git, &target, now + 1)?;
            syncs.push(started.elapsed());
        }
    }
    report(
        lab,
        "Quire notebook (format 1 change files)",
        &remote,
        &computers[0].0,
        Measured { syncs, publishes },
    )
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let days: u64 = args
        .next()
        .map_or(Ok(365), |days| days.parse())
        .context("days must be a number")?;
    let parent = args.next().map_or_else(std::env::temp_dir, PathBuf::from);
    let directory = tempfile::Builder::new()
        .prefix("quire-git-growth-")
        .tempdir_in(parent)?;
    let config = directory.path().join("gitconfig");
    fs::write(
        &config,
        "[user]\n\tname = Growth\n\temail = growth@example.invalid\n[commit]\n\tgpgsign = false\n",
    )?;
    let git = Git::default()
        .env("GIT_CONFIG_GLOBAL", &config)
        .env("GIT_CONFIG_NOSYSTEM", "1");
    let lab = Lab {
        directory: fs::canonicalize(directory.path())?,
        git,
        config,
    };
    println!(
        "{days} days, git {}",
        lab.run(&lab.directory, &["--version"])?
    );
    let started = Instant::now();
    folder_year(&lab, days)?;
    quire_year(&lab, days)?;
    println!("total {:.1} s", started.elapsed().as_secs_f64());
    Ok(())
}
