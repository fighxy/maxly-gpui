use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex, PoisonError};

use bytes::Bytes;
use max_identity::Environment;
use max_store::Profile;
use max_transport::{Incoming, Response, Transport, connect, tls_config_for};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;
use tracing::{debug, info};

use crate::{CoreConfig, CoreError, LoginResponse, Result, Session, SyncState};

/// A byte stream the core can run a session over.
pub trait Stream: AsyncRead + AsyncWrite + Send + Unpin + 'static {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin + 'static> Stream for T {}

/// The future [`Connector::connect`] returns.
pub type ConnectFuture<'a> = Pin<Box<dyn Future<Output = Result<Box<dyn Stream>>> + Send + 'a>>;

/// Opens connections for the [`Supervisor`].
pub trait Connector: Send + Sync + 'static {
    /// Open a new connection.
    fn connect(&self) -> ConnectFuture<'_>;
}

/// TLS to the configured endpoint.
pub struct TlsConnector {
    config: CoreConfig,
}

impl TlsConnector {
    /// A connector for `config.endpoint`.
    pub fn new(config: CoreConfig) -> TlsConnector {
        TlsConnector { config }
    }
}

impl Connector for TlsConnector {
    fn connect(&self) -> ConnectFuture<'_> {
        Box::pin(async move {
            let stream = connect(
                &self.config.endpoint,
                tls_config_for(&self.config.endpoint.host)?,
                self.config.connect_timeout,
            )
            .await?;
            Ok(Box::new(stream) as Box<dyn Stream>)
        })
    }
}

/// What the supervisor reports.
#[derive(Debug)]
pub enum CoreEvent {
    /// `SESSION_INIT` succeeded on a new connection.
    Connected,
    /// `LOGIN` succeeded; the initial sync.
    LoggedIn(Box<LoginResponse>),
    /// The profile has no token: log in through a [`Session`] first.
    AuthRequired,
    /// A server push.
    Push(Response),
    /// The connection ended; the supervisor reconnects after a delay.
    Disconnected(String),
}

/// Requests through whatever connection is current.
#[derive(Clone)]
pub struct SupervisorHandle {
    current: Arc<Mutex<Option<Transport>>>,
    stop: CancellationToken,
}

impl SupervisorHandle {
    /// The current logged-in connection; [`max_transport::TransportError::Closed`] while
    /// reconnecting.
    pub fn transport(&self) -> Result<Transport> {
        self.current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .clone()
            .ok_or(CoreError::Transport(max_transport::TransportError::Closed))
    }

    /// Send a request on the current connection.
    pub async fn request(&self, opcode: u16, payload: impl Into<Bytes>) -> Result<Response> {
        Ok(self.transport()?.request(opcode, payload).await?)
    }

    /// Stop the supervisor and close the connection.
    pub fn stop(&self) {
        self.stop.cancel();
        if let Some(t) = self
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .take()
        {
            t.close();
        }
    }
}

/// Keeps a logged-in session running: connect, `SESSION_INIT`, `LOGIN` with the saved token,
/// forward pushes, and on any loss start again after the backoff delay.
pub struct Supervisor;

impl Supervisor {
    /// Start supervising `profile`. The task owns the profile until it stops.
    pub fn spawn(
        config: CoreConfig,
        connector: Arc<dyn Connector>,
        profile: Profile,
        env: Environment,
    ) -> (SupervisorHandle, mpsc::UnboundedReceiver<CoreEvent>) {
        let (events, rx) = mpsc::unbounded_channel();
        let handle = SupervisorHandle {
            current: Arc::new(Mutex::new(None)),
            stop: CancellationToken::new(),
        };
        tokio::spawn(run(config, connector, profile, env, handle.clone(), events));
        (handle, rx)
    }
}

async fn run(
    config: CoreConfig,
    connector: Arc<dyn Connector>,
    mut profile: Profile,
    env: Environment,
    handle: SupervisorHandle,
    events: mpsc::UnboundedSender<CoreEvent>,
) {
    let mut backoff = config.backoff.clone();
    let sync = SyncState::default();
    while !handle.stop.is_cancelled() {
        let reason = match one_connection(
            &config,
            connector.as_ref(),
            &mut profile,
            &env,
            &sync,
            &handle,
            &events,
        )
        .await
        {
            Ok(Outcome::NoToken) => {
                let _ = events.send(CoreEvent::AuthRequired);
                return;
            }
            Ok(Outcome::Ended(reason)) => {
                backoff.reset();
                reason
            }
            Err(e) => e.to_string(),
        };
        *handle
            .current
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = None;
        if handle.stop.is_cancelled() {
            return;
        }
        let _ = events.send(CoreEvent::Disconnected(reason));
        let delay = backoff.next_delay();
        debug!(?delay, "reconnecting");
        tokio::select! {
            () = handle.stop.cancelled() => return,
            () = tokio::time::sleep(delay) => {}
        }
    }
}

enum Outcome {
    NoToken,
    Ended(String),
}

async fn one_connection(
    config: &CoreConfig,
    connector: &dyn Connector,
    profile: &mut Profile,
    env: &Environment,
    sync: &SyncState,
    handle: &SupervisorHandle,
    events: &mpsc::UnboundedSender<CoreEvent>,
) -> Result<Outcome> {
    let Some(token) = profile.auth_token()? else {
        return Ok(Outcome::NoToken);
    };
    let stream = connector.connect().await?;
    let session = Session::start(stream, config, profile, env).await?;
    let _ = events.send(CoreEvent::Connected);
    let login = session
        .login(&token, sync, config.transport.interactive)
        .await?;
    info!("logged in");
    let _ = events.send(CoreEvent::LoggedIn(Box::new(login)));
    let (transport, mut pushes) = session.into_parts();
    *handle
        .current
        .lock()
        .unwrap_or_else(PoisonError::into_inner) = Some(transport);
    loop {
        tokio::select! {
            () = handle.stop.cancelled() => return Ok(Outcome::Ended("stopped".into())),
            item = pushes.recv() => match item {
                Some(Incoming::Push(p)) => { let _ = events.send(CoreEvent::Push(p)); }
                Some(Incoming::Closed(err)) => {
                    return Ok(Outcome::Ended(err.map_or_else(|| "closed".to_string(), |e| e.to_string())));
                }
                None => return Ok(Outcome::Ended("closed".into())),
            },
        }
    }
}
