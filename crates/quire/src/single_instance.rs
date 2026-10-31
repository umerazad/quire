//! One Quire process per application data folder, as the reference app's single-instance lock
//! provides. A second launch hands its arguments to the running Quire over a local socket in the
//! data folder and exits; the running Quire opens what was asked for or comes forward. A newer
//! build takes over instead: the running Quire saves, quits, and starts it. While the running
//! Quire is quitting it answers "busy", and the newer one waits for the socket to close.
//!
//! The socket follows Zed's Linux listener (`zed-<channel>.sock` in the data folder, a stale file
//! removed when nothing answers). On Windows Quire does not hand over yet; the notebook lock still
//! keeps a second process out of an open notebook.

use serde::{Deserialize, Serialize};
use std::{
    io::{self, BufRead as _, BufReader, Write as _},
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

/// What a launch asks of the running Quire.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Launch {
    pub version: String,
    pub executable: PathBuf,
    pub args: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Reply {
    /// The running Quire opens the arguments or comes forward.
    Focus,
    /// The running Quire quits and starts the newer build.
    Handover,
    /// The running Quire is quitting; try again shortly.
    Busy,
}

fn parts(version: &str) -> [u64; 3] {
    let mut parts = version.split(['.', '+', '-']).map(|part| {
        part.chars()
            .take_while(char::is_ascii_digit)
            .collect::<String>()
            .parse()
            .unwrap_or(0)
    });
    [(); 3].map(|()| parts.next().unwrap_or(0))
}

/// Whether `candidate` is a newer release than `current`, comparing major, minor, and patch.
pub fn is_newer(candidate: &str, current: &str) -> bool {
    parts(candidate) > parts(current)
}

/// What the running Quire does for a launch: a newer build takes over, anything else focuses.
pub fn reply_for(current: &str, launch: &Launch) -> Reply {
    if is_newer(&launch.version, current) {
        Reply::Handover
    } else {
        Reply::Focus
    }
}

pub enum Claim {
    /// This is the only Quire: serve later launches with the listener.
    First(Listener),
    /// Another Quire answered.
    Running(Reply),
}

#[cfg(unix)]
pub struct Listener(std::os::unix::net::UnixListener);

#[cfg(not(unix))]
pub struct Listener;

/// Claims the data folder's socket for this launch, or hands the launch to the Quire holding it,
/// retrying `attempts` times `delay` apart while that Quire is quitting.
#[cfg(unix)]
pub fn claim(socket: &Path, launch: &Launch, attempts: u32, delay: Duration) -> io::Result<Claim> {
    use std::os::unix::net::{UnixListener, UnixStream};
    for _ in 0..attempts {
        match UnixStream::connect(socket) {
            Ok(stream) => match ask(stream, launch)? {
                Reply::Busy => std::thread::sleep(delay),
                reply => return Ok(Claim::Running(reply)),
            },
            Err(error)
                if matches!(
                    error.kind(),
                    io::ErrorKind::NotFound | io::ErrorKind::ConnectionRefused
                ) =>
            {
                match std::fs::remove_file(socket) {
                    Err(error) if error.kind() != io::ErrorKind::NotFound => return Err(error),
                    _ => {}
                }
                match UnixListener::bind(socket) {
                    Ok(listener) => return Ok(Claim::First(Listener(listener))),
                    Err(error) if error.kind() == io::ErrorKind::AddrInUse => {}
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
    }
    Err(io::Error::new(
        io::ErrorKind::TimedOut,
        "the running Quire kept answering that it is quitting",
    ))
}

#[cfg(not(unix))]
pub fn claim(_: &Path, _: &Launch, _: u32, _: Duration) -> io::Result<Claim> {
    Ok(Claim::First(Listener))
}

#[cfg(unix)]
fn ask(stream: std::os::unix::net::UnixStream, launch: &Launch) -> io::Result<Reply> {
    stream.set_read_timeout(Some(Duration::from_secs(5)))?;
    let mut writer = &stream;
    writer.write_all(&serde_json::to_vec(launch).map_err(io::Error::other)?)?;
    writer.write_all(b"\n")?;
    let mut line = String::new();
    BufReader::new(&stream).read_line(&mut line)?;
    serde_json::from_str(line.trim()).map_err(io::Error::other)
}

impl Listener {
    /// Answers launches on a background thread and passes each one that is not refused as busy
    /// to `deliver`. Once `quitting` is set, every launch is told to wait.
    #[cfg(unix)]
    pub fn serve(
        self,
        current: String,
        quitting: Arc<AtomicBool>,
        deliver: impl Fn(Launch, Reply) + Send + 'static,
    ) -> io::Result<()> {
        std::thread::Builder::new()
            .name("QuireSingleInstance".into())
            .spawn(move || {
                for stream in self.0.incoming() {
                    let Ok(stream) = stream else {
                        continue;
                    };
                    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
                    let mut line = String::new();
                    if BufReader::new(&stream).read_line(&mut line).is_err() {
                        continue;
                    }
                    let Ok(launch) = serde_json::from_str::<Launch>(line.trim()) else {
                        continue;
                    };
                    let reply = if quitting.load(Ordering::SeqCst) {
                        Reply::Busy
                    } else {
                        reply_for(&current, &launch)
                    };
                    let mut writer = &stream;
                    let answered = serde_json::to_vec(&reply)
                        .map_err(io::Error::other)
                        .and_then(|bytes| writer.write_all(&[bytes, b"\n".to_vec()].concat()));
                    if answered.is_ok() && reply != Reply::Busy {
                        deliver(launch, reply);
                    }
                }
            })
            .map(drop)
    }

    #[cfg(not(unix))]
    pub fn serve(
        self,
        _: String,
        _: Arc<AtomicBool>,
        _: impl Fn(Launch, Reply) + Send + 'static,
    ) -> io::Result<()> {
        Ok(())
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::sync::mpsc;

    fn launch(version: &str, args: &[&str]) -> Launch {
        Launch {
            version: version.into(),
            executable: "/Applications/Quire.app/Contents/MacOS/quire".into(),
            args: args.iter().map(|arg| arg.to_string()).collect(),
        }
    }

    const QUICK: Duration = Duration::from_millis(20);

    #[test]
    fn versions_compare_by_release_number() {
        assert!(is_newer("0.10.0", "0.9.57"));
        assert!(is_newer("1.0.0-beta", "0.99.99"));
        assert!(!is_newer("0.9.57", "0.9.57"));
        assert!(!is_newer("0.9.5", "0.9.57"));
        assert!(!is_newer("garbage", "0.1.0"));
        assert_eq!(reply_for("0.1.0", &launch("0.2.0", &[])), Reply::Handover);
        assert_eq!(reply_for("0.2.0", &launch("0.1.0", &[])), Reply::Focus);
    }

    #[test]
    fn a_second_launch_hands_its_arguments_to_the_first_and_a_newer_one_takes_over() {
        let folder = tempfile::tempdir().unwrap();
        let socket = folder.path().join("q.sock");
        let Claim::First(listener) = claim(&socket, &launch("0.1.0", &[]), 1, QUICK).unwrap()
        else {
            panic!("the first launch must own the socket");
        };
        let (sender, received) = mpsc::channel();
        let quitting = Arc::new(AtomicBool::new(false));
        listener
            .serve("0.1.0".into(), quitting.clone(), move |launch, reply| {
                sender.send((launch, reply)).unwrap()
            })
            .unwrap();
        let second = launch("0.1.0", &["--notebook-folder", "/notes"]);
        assert!(matches!(
            claim(&socket, &second, 1, QUICK).unwrap(),
            Claim::Running(Reply::Focus)
        ));
        assert_eq!(received.recv().unwrap(), (second, Reply::Focus));
        let newer = launch("0.2.0", &[]);
        assert!(matches!(
            claim(&socket, &newer, 1, QUICK).unwrap(),
            Claim::Running(Reply::Handover)
        ));
        assert_eq!(received.recv().unwrap(), (newer.clone(), Reply::Handover));

        quitting.store(true, Ordering::SeqCst);
        let error = claim(&socket, &newer, 3, QUICK).err().unwrap();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
        assert!(received.try_recv().is_err());
    }

    #[test]
    fn a_socket_left_by_a_quit_or_crashed_quire_is_claimed_again() {
        let folder = tempfile::tempdir().unwrap();
        let socket = folder.path().join("q.sock");
        let first = claim(&socket, &launch("0.1.0", &[]), 1, QUICK).unwrap();
        drop(first);
        assert!(socket.exists());
        assert!(matches!(
            claim(&socket, &launch("0.1.0", &[]), 1, QUICK).unwrap(),
            Claim::First(_)
        ));
    }
}
