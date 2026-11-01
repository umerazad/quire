//! One Quire process per application data folder, as the reference app's single-instance lock
//! provides. A second launch hands its arguments to the running Quire over a local socket in the
//! data folder and exits; the running Quire opens what was asked for or comes forward. A newer
//! build takes over instead: the running Quire saves, quits, and starts it. While the running
//! Quire is quitting it answers "busy", and the newer one waits for the socket to close.
//!
//! The socket follows Zed's Linux listener (`zed-<channel>.sock` in the data folder, a stale file
//! removed when nothing answers). It is a Unix domain socket on every OS through Zed's `net` crate,
//! which Zed also uses for its MCP server and askpass sockets on Windows (AF_UNIX, Windows 10 1803
//! and later), so Windows speaks the same Launch and Reply lines as macOS and Linux.

use net::{UnixListener, UnixStream};
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

/// The socket `name` in `folder`, or one in the temporary folder when that path is too long to
/// bind (about 100 bytes). The same folder and name always give the same socket, so a later launch
/// finds the running Quire.
pub fn socket_path(folder: &Path, name: &str) -> PathBuf {
    use sha2::{Digest as _, Sha256};
    let inside = folder.join(name);
    if inside.as_os_str().len() < 100 {
        return inside;
    }
    let hash = Sha256::digest(inside.as_os_str().as_encoded_bytes());
    std::env::temp_dir().join(format!("quire-{}.sock", &hex::encode(hash)[..16]))
}

pub enum Claim {
    /// This is the only Quire: serve later launches with the listener.
    First(Listener),
    /// Another Quire answered.
    Running(Reply),
}

pub struct Listener(UnixListener);

/// How long either side waits for the other's line before giving up on the connection.
const ANSWER_WITHIN: Duration = Duration::from_secs(5);

/// Claims the data folder's socket for this launch, or hands the launch to the Quire holding it,
/// retrying `attempts` times `delay` apart while that Quire is quitting.
pub fn claim(socket: &Path, launch: &Launch, attempts: u32, delay: Duration) -> io::Result<Claim> {
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
                ) || !socket.exists() =>
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

/// Reads one line from `stream` on a helper thread, giving up after `within`. Windows sockets
/// from Zed's `net` crate have no read timeout, so the wait is bounded here on every platform.
fn read_line_within(
    stream: impl io::Read + Send + 'static,
    within: Duration,
) -> io::Result<String> {
    let (send, receive) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("QuireSocketRead".into())
        .spawn(move || {
            let mut line = String::new();
            let read = BufReader::new(stream).read_line(&mut line);
            send.send(read.map(|_| line)).ok();
        })?;
    receive.recv_timeout(within).unwrap_or_else(|_| {
        Err(io::Error::new(
            io::ErrorKind::TimedOut,
            "the other side did not answer in time",
        ))
    })
}

fn ask(mut stream: UnixStream, launch: &Launch) -> io::Result<Reply> {
    let mut request = serde_json::to_vec(launch).map_err(io::Error::other)?;
    request.push(b'\n');
    stream.write_all(&request)?;
    let line = read_line_within(stream, ANSWER_WITHIN)?;
    serde_json::from_str(line.trim()).map_err(io::Error::other)
}

impl Listener {
    /// Answers launches on a background thread, each connection on its own thread so a client
    /// that never finishes its line holds up no one, and passes each launch that is not refused
    /// as busy to `deliver`. Once `quitting` is set, every launch is told to wait.
    pub fn serve(
        self,
        current: String,
        quitting: Arc<AtomicBool>,
        deliver: impl Fn(Launch, Reply) + Send + Sync + 'static,
    ) -> io::Result<()> {
        let deliver = Arc::new(deliver);
        std::thread::Builder::new()
            .name("QuireSingleInstance".into())
            .spawn(move || {
                while let Ok((stream, _)) = self.0.accept() {
                    let (current, quitting, deliver) =
                        (current.clone(), quitting.clone(), deliver.clone());
                    std::thread::Builder::new()
                        .name("QuireLaunch".into())
                        .spawn(move || answer(stream, &current, &quitting, &*deliver))
                        .ok();
                }
            })
            .map(drop)
    }
}

fn answer(
    mut stream: UnixStream,
    current: &str,
    quitting: &AtomicBool,
    deliver: &dyn Fn(Launch, Reply),
) {
    let mut line = String::new();
    if BufReader::new(&mut stream).read_line(&mut line).is_err() {
        return;
    }
    let Ok(launch) = serde_json::from_str::<Launch>(line.trim()) else {
        return;
    };
    let reply = if quitting.load(Ordering::SeqCst) {
        Reply::Busy
    } else {
        reply_for(current, &launch)
    };
    let answered = serde_json::to_vec(&reply)
        .map_err(io::Error::other)
        .and_then(|mut bytes| {
            bytes.push(b'\n');
            stream.write_all(&bytes)
        });
    if answered.is_ok() && reply != Reply::Busy {
        deliver(launch, reply);
    }
}
#[cfg(test)]
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
    fn a_launch_that_never_finishes_its_line_holds_up_no_other_launch() {
        let folder = tempfile::tempdir().unwrap();
        let socket = folder.path().join("q.sock");
        let Claim::First(listener) = claim(&socket, &launch("0.1.0", &[]), 1, QUICK).unwrap()
        else {
            panic!("the first launch must own the socket");
        };
        let (sender, received) = mpsc::channel();
        listener
            .serve(
                "0.1.0".into(),
                Arc::new(AtomicBool::new(false)),
                move |launch, reply| sender.send((launch, reply)).unwrap(),
            )
            .unwrap();
        let _silent = UnixStream::connect(&socket).unwrap();
        let second = launch("0.1.0", &["--notebook-folder", "/notes"]);
        assert!(matches!(
            claim(&socket, &second, 1, QUICK).unwrap(),
            Claim::Running(Reply::Focus)
        ));
        assert_eq!(received.recv().unwrap(), (second, Reply::Focus));
    }

    #[test]
    fn a_running_quire_that_never_answers_is_given_up_on() {
        let folder = tempfile::tempdir().unwrap();
        let socket = folder.path().join("q.sock");
        let _mute = UnixListener::bind(&socket).unwrap();
        let mut stream = UnixStream::connect(&socket).unwrap();
        stream.write_all(b"{}\n").unwrap();
        let error = read_line_within(stream, QUICK).unwrap_err();
        assert_eq!(error.kind(), io::ErrorKind::TimedOut);
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

    #[test]
    fn long_data_folders_put_the_socket_in_the_temporary_folder() {
        let folder = tempfile::tempdir().unwrap();
        let long = folder.path().join("a".repeat(120));
        let socket = socket_path(&long, "quire.sock");
        assert!(socket.starts_with(std::env::temp_dir()));
        assert_eq!(socket, socket_path(&long, "quire.sock"));
        assert_ne!(socket, socket_path(&long, "bridge.sock"));
        assert_eq!(
            socket_path(Path::new("/tmp/q"), "quire.sock"),
            PathBuf::from("/tmp/q/quire.sock")
        );
        let first = claim(&socket, &launch("0.1.0", &[]), 1, QUICK).unwrap();
        assert!(matches!(first, Claim::First(_)));
        drop(first);
        std::fs::remove_file(&socket).ok();
    }
}
