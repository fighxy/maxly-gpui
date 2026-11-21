//! One running client per Windows user (M3). The first instance holds a named pipe; a second
//! one connects to it, asks it to show its window, and exits. The pipe name carries the user
//! name so other accounts on the machine have their own; tokio rejects remote clients.

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
    /// Holds the thread that serves the pipe.
    _runtime: tokio::runtime::Runtime,
    /// One message per later start of the client.
    pub wakeups: std::sync::mpsc::Receiver<()>,
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

/// No single-instance check outside Windows yet.
#[cfg(not(windows))]
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
}
