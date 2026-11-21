//! One running client per user (M3). The first instance holds an endpoint; a second one
//! connects to it, asks it to show its window, and exits. Windows uses a named pipe (tokio
//! rejects remote clients). Linux uses a socket in `$XDG_RUNTIME_DIR`, which systemd already
//! keeps private to the user — the usual place on Arch.

/// The outcome of [`claim`].
pub enum Claim {
    /// We are the only instance; `wakeups` gets a message each time another one starts.
    First(Instance),
    /// Another instance runs and was asked to show itself: exit.
    Running,
    /// No pipe could be made (or on other platforms): run without the check.
    Alone,
}

/// Keeps the pipe served for as long as the app runs.
pub struct Instance {
    /// Holds the thread that serves the endpoint.
    _runtime: tokio::runtime::Runtime,
    /// One message per later start of the client.
    pub wakeups: std::sync::mpsc::Receiver<()>,
    /// Socket file to remove when this instance exits. A crash leaves it; the next start
    /// removes a socket nobody answers.
    #[cfg(unix)]
    socket: std::path::PathBuf,
}

#[cfg(unix)]
impl Drop for Instance {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.socket);
    }
}

/// `\\.\pipe\maxly-gpui-<user>-<hash>`: the user name cut to ASCII letters, digits, `-` and `_`, and
/// an FNV-1a hash of the whole name, so names that differ only in other letters (Cyrillic ones)
/// stay apart. FNV is fixed, unlike std's hasher, so every build finds the same pipe.
pub fn pipe_name(user: &str) -> String {
    let short: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    let hash = user.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| {
        (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3)
    });
    format!(r"\\.\pipe\maxly-gpui-{short}-{hash:016x}")
}

/// What a second instance sends.
const SHOW: &[u8] = b"show";

/// Become the instance, or tell the running one to show itself.
#[cfg(windows)]
pub fn claim() -> Claim {
    claim_pipe(pipe_name(&std::env::var("USERNAME").unwrap_or_default()))
}

/// [`claim`] on the pipe `name`.
#[cfg(windows)]
fn claim_pipe(name: String) -> Claim {
    use tokio::io::AsyncWriteExt as _;
    use tokio::net::windows::named_pipe::{ClientOptions, ServerOptions};

    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("maxly-gpui-instance")
        .enable_all()
        .build()
    else {
        return Claim::Alone;
    };
    let first =
        runtime.block_on(async { ServerOptions::new().first_pipe_instance(true).create(&name) });
    match first {
        Ok(server) => {
            let (tx, wakeups) = std::sync::mpsc::channel();
            runtime.spawn(serve(server, name, tx));
            Claim::First(Instance {
                _runtime: runtime,
                wakeups,
            })
        }
        Err(_) => {
            // The name is taken: reach the running instance (retrying while its pipe is busy).
            let told = runtime.block_on(async {
                for _ in 0..20 {
                    if let Ok(mut client) = ClientOptions::new().open(&name) {
                        return client.write_all(SHOW).await.is_ok();
                    }
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
                false
            });
            if told { Claim::Running } else { Claim::Alone }
        }
    }
}

#[cfg(windows)]
async fn serve(
    mut server: tokio::net::windows::named_pipe::NamedPipeServer,
    name: String,
    wakeups: std::sync::mpsc::Sender<()>,
) {
    use tokio::io::AsyncReadExt as _;
    use tokio::net::windows::named_pipe::ServerOptions;

    loop {
        if server.connect().await.is_err() {
            return;
        }
        // The next client needs a fresh pipe instance before this one is read.
        let next = ServerOptions::new().create(&name);
        let mut buf = [0u8; 16];
        if let Ok(n) = server.read(&mut buf).await
            && buf.get(..n) == Some(SHOW)
            && wakeups.send(()).is_err()
        {
            return;
        }
        match next {
            Ok(pipe) => server = pipe,
            Err(_) => return,
        }
    }
}

/// `$XDG_RUNTIME_DIR/maxly-gpui.sock`, or a `0700` directory under the temp dir when the
/// session has no runtime directory (a bare `su`, not a normal Arch login).
#[cfg(unix)]
fn runtime_socket() -> std::path::PathBuf {
    if let Some(dir) = std::env::var_os("XDG_RUNTIME_DIR").filter(|d| !d.is_empty()) {
        return std::path::PathBuf::from(dir).join("maxly-gpui.sock");
    }
    let user = std::env::var("USER").unwrap_or_else(|_| "user".into());
    let short: String = user
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_')
        .take(32)
        .collect();
    let dir = std::env::temp_dir().join(format!("maxly-gpui-{short}"));
    use std::os::unix::fs::DirBuilderExt as _;
    let mut builder = std::fs::DirBuilder::new();
    let _ = builder.mode(0o700).recursive(true).create(&dir);
    dir.join("maxly-gpui.sock")
}

/// Become the instance on a Unix socket, or tell the running one to show itself.
#[cfg(unix)]
pub fn claim() -> Claim {
    claim_socket(runtime_socket())
}

/// [`claim`] on the socket `path`.
#[cfg(unix)]
fn claim_socket(path: std::path::PathBuf) -> Claim {
    use tokio::io::AsyncReadExt as _;
    use tokio::net::UnixListener;

    if poke(&path) {
        return Claim::Running;
    }
    // A previous crash leaves the file. Nobody answered, so it is ours to replace.
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
    if let Some(dir) = path.parent()
        && !dir.as_os_str().is_empty()
        && std::fs::create_dir_all(dir).is_err()
    {
        return Claim::Alone;
    }
    let Ok(listener) = UnixListener::bind(&path) else {
        return if poke(&path) {
            Claim::Running
        } else {
            Claim::Alone
        };
    };
    let Ok(runtime) = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(1)
        .thread_name("maxly-gpui-instance")
        .enable_all()
        .build()
    else {
        let _ = std::fs::remove_file(&path);
        return Claim::Alone;
    };
    let (tx, wakeups) = std::sync::mpsc::channel();
    runtime.spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let mut buf = [0u8; 16];
            if let Ok(n) = stream.read(&mut buf).await
                && buf.get(..n) == Some(SHOW)
                && tx.send(()).is_err()
            {
                return;
            }
        }
    });
    Claim::First(Instance {
        _runtime: runtime,
        wakeups,
        socket: path,
    })
}

/// `true` when a living instance accepted `show`.
#[cfg(unix)]
fn poke(path: &std::path::Path) -> bool {
    use std::io::Write as _;
    let Ok(mut stream) = std::os::unix::net::UnixStream::connect(path) else {
        return false;
    };
    let _ = stream.set_write_timeout(Some(std::time::Duration::from_millis(500)));
    stream.write_all(SHOW).is_ok()
}

/// No single-instance check on platforms without a pipe or a socket.
#[cfg(not(any(windows, unix)))]
pub fn claim() -> Claim {
    Claim::Alone
}

#[cfg(test)]
mod tests {
    use super::pipe_name;

    #[test]
    fn pipe_names_are_per_user_and_clean() {
        assert_eq!(pipe_name("ivan"), pipe_name("ivan"), "stable");
        assert!(pipe_name("ivan").starts_with(r"\\.\pipe\maxly-gpui-ivan-"));
        assert!(pipe_name(r"Иван\..\x y").starts_with(r"\\.\pipe\maxly-gpui-xy-"));
        assert_ne!(
            pipe_name("Иван"),
            pipe_name("Пётр"),
            "non-ASCII names stay apart"
        );
        // FNV-1a of the empty string is its offset basis.
        assert_eq!(pipe_name(""), r"\\.\pipe\maxly-gpui--cbf29ce484222325");
    }

    /// The real pipe, under a name of its own: the first claim serves it, a second one finds it
    /// taken and wakes the first, twice in a row.
    #[cfg(windows)]
    #[test]
    fn second_start_wakes_the_first() {
        use std::time::Duration;

        use super::{Claim, claim_pipe};

        let name = format!(r"\\.\pipe\maxly-gpui-test-{}", std::process::id());
        let Claim::First(first) = claim_pipe(name.clone()) else {
            panic!("the first claim must own the pipe");
        };
        for _ in 0..2 {
            assert!(matches!(claim_pipe(name.clone()), Claim::Running));
            assert_eq!(first.wakeups.recv_timeout(Duration::from_secs(5)), Ok(()));
        }
    }

    /// The real socket, under a name of its own: the first claim serves it, a second one finds it
    /// and wakes the first, twice in a row.
    #[cfg(unix)]
    #[test]
    fn second_start_wakes_the_first() {
        use std::time::Duration;

        use super::{Claim, claim_socket};

        let path =
            std::env::temp_dir().join(format!("maxly-gpui-test-{}.sock", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let Claim::First(first) = claim_socket(path.clone()) else {
            panic!("the first claim must own the socket");
        };
        for _ in 0..2 {
            assert!(matches!(claim_socket(path.clone()), Claim::Running));
            assert_eq!(first.wakeups.recv_timeout(Duration::from_secs(5)), Ok(()));
        }
    }
}
