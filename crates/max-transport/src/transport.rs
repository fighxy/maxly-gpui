use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard, PoisonError, Weak};
use std::time::Duration;

use bytes::{Bytes, BytesMut};
use max_proto::packets::Ping;
use max_proto::payload::{decode_value, from_msgpack, to_msgpack};
use max_proto::{Cmd, Frame, FrameDecoder, Header, MAX_PLAIN_LEN, Opcode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{Instant, MissedTickBehavior};
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::{Result, TransportError};

/// Connection settings. Defaults follow the desktop client's `config.dll`.
#[derive(Debug, Clone)]
pub struct TransportConfig {
    /// How long a request waits for its response (`check_connection_timeout`: 30 s).
    pub request_timeout: Duration,
    /// `PING` period (`ping_interval`: 30 000 ms); `None` disables pings.
    pub ping_interval: Option<Duration>,
    /// Value of `PING.interactive`.
    pub interactive: bool,
    /// Largest decompressed payload accepted from the server.
    pub max_plain: usize,
}

impl Default for TransportConfig {
    fn default() -> TransportConfig {
        TransportConfig {
            request_timeout: Duration::from_secs(30),
            ping_interval: Some(Duration::from_secs(30)),
            interactive: true,
            max_plain: MAX_PLAIN_LEN,
        }
    }
}

/// A server frame with its payload decompressed.
#[derive(Debug, Clone, PartialEq)]
pub struct Response {
    /// The frame header as received (`payload_len` is the wire length).
    pub header: Header,
    /// Decompressed msgpack payload; empty for an empty response.
    pub payload: Bytes,
}

impl Response {
    /// Decode the payload into a type.
    pub fn decode<T: DeserializeOwned>(&self) -> Result<T> {
        Ok(from_msgpack(&self.payload)?)
    }

    /// Decode the payload into a value tree.
    pub fn value(&self) -> Result<rmpv::Value> {
        Ok(decode_value(&self.payload)?)
    }
}

/// What the connection hands to its owner besides responses.
#[derive(Debug)]
pub enum Incoming {
    /// A frame that answers no pending request: notifications, events, server requests.
    Push(Response),
    /// The connection ended; `None` when it was closed by either side without an error.
    Closed(Option<TransportError>),
}

type Reply = oneshot::Sender<Result<Response>>;

struct State {
    next_seq: u16,
    pending: HashMap<u16, (u16, Reply)>,
    closed: bool,
}

struct Inner {
    state: Arc<Mutex<State>>,
    out: mpsc::Sender<Bytes>,
    stop: CancellationToken,
    config: TransportConfig,
}

impl Drop for Inner {
    fn drop(&mut self) {
        self.stop.cancel();
    }
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    // A panic while holding the lock leaves only plain data behind; keep using it.
    state.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Handle to a running connection. Cheap to clone; the connection stops when the last handle
/// is dropped or [`Transport::close`] is called.
#[derive(Clone)]
pub struct Transport {
    inner: Arc<Inner>,
}

impl Transport {
    /// Run a connection over `stream` (TLS in production, any duplex stream in tests).
    /// Returns the handle and the receiver of [`Incoming`] items; drain the receiver, it is
    /// unbounded so that pushes never block responses.
    pub fn spawn<S>(
        stream: S,
        config: TransportConfig,
    ) -> (Transport, mpsc::UnboundedReceiver<Incoming>)
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (rd, wr) = tokio::io::split(stream);
        let (out_tx, out_rx) = mpsc::channel::<Bytes>(64);
        let (in_tx, in_rx) = mpsc::unbounded_channel();
        let stop = CancellationToken::new();
        let state = Arc::new(Mutex::new(State {
            next_seq: 1,
            pending: HashMap::new(),
            closed: false,
        }));
        tokio::spawn(writer(wr, out_rx, stop.clone()));
        tokio::spawn(reader(
            rd,
            state.clone(),
            in_tx,
            config.max_plain,
            stop.clone(),
        ));
        let ping = config.ping_interval;
        let interactive = config.interactive;
        let inner = Arc::new(Inner {
            state,
            out: out_tx,
            stop: stop.clone(),
            config,
        });
        if let Some(every) = ping {
            tokio::spawn(pinger(Arc::downgrade(&inner), every, interactive, stop));
        }
        (Transport { inner }, in_rx)
    }

    /// Send a request with a ready msgpack payload and wait for its response.
    pub async fn request(&self, opcode: u16, payload: impl Into<Bytes>) -> Result<Response> {
        let (tx, rx) = oneshot::channel();
        let seq = {
            let mut st = lock(&self.inner.state);
            if st.closed {
                return Err(TransportError::Closed);
            }
            let seq = next_free_seq(&mut st)?;
            st.pending.insert(seq, (opcode, tx));
            seq
        };
        let sent = match Frame::request(seq, opcode, payload) {
            Ok(frame) => self
                .inner
                .out
                .send(frame.encode())
                .await
                .map_err(|_| TransportError::Closed),
            Err(e) => Err(e.into()),
        };
        if let Err(e) = sent {
            lock(&self.inner.state).pending.remove(&seq);
            return Err(e);
        }
        match tokio::time::timeout(self.inner.config.request_timeout, rx).await {
            Ok(Ok(result)) => result,
            Ok(Err(_)) => Err(TransportError::Closed),
            Err(_) => {
                lock(&self.inner.state).pending.remove(&seq);
                Err(TransportError::Timeout { opcode, seq })
            }
        }
    }

    /// Send a typed request and decode its response.
    pub async fn call<Req, Resp>(&self, opcode: Opcode, request: &Req) -> Result<Resp>
    where
        Req: Serialize + ?Sized,
        Resp: DeserializeOwned,
    {
        let payload = to_msgpack(request)?;
        self.request(opcode.as_u16(), payload).await?.decode()
    }

    /// Close the connection. Pending requests fail with [`TransportError::Closed`].
    pub fn close(&self) {
        self.inner.stop.cancel();
    }

    /// Whether the connection has ended.
    pub fn is_closed(&self) -> bool {
        lock(&self.inner.state).closed || self.inner.stop.is_cancelled()
    }

    /// Number of requests waiting for a response.
    pub fn pending(&self) -> usize {
        lock(&self.inner.state).pending.len()
    }
}

/// Next unused seq: counts up from 1, wraps past 65535 back to 1 (0 is never used).
fn next_free_seq(st: &mut State) -> Result<u16> {
    for _ in 0..u16::MAX {
        let seq = st.next_seq;
        st.next_seq = if seq == u16::MAX { 1 } else { seq + 1 };
        if !st.pending.contains_key(&seq) {
            return Ok(seq);
        }
    }
    Err(TransportError::SeqExhausted)
}

async fn writer<W: AsyncWrite>(wr: W, mut out: mpsc::Receiver<Bytes>, stop: CancellationToken) {
    let mut wr = Box::pin(wr);
    loop {
        tokio::select! {
            () = stop.cancelled() => break,
            next = out.recv() => match next {
                Some(bytes) => {
                    if let Err(e) = async {
                        wr.write_all(&bytes).await?;
                        wr.flush().await
                    }
                    .await
                    {
                        debug!(error = %e, "write failed");
                        stop.cancel();
                        break;
                    }
                }
                None => break,
            },
        }
    }
    let _ = wr.shutdown().await;
}

async fn reader<R: AsyncRead>(
    rd: R,
    state: Arc<Mutex<State>>,
    incoming: mpsc::UnboundedSender<Incoming>,
    max_plain: usize,
    stop: CancellationToken,
) {
    let mut rd = Box::pin(rd);
    let mut buf = BytesMut::with_capacity(16 * 1024);
    let mut decoder = FrameDecoder::new();
    let outcome: Option<TransportError> = loop {
        while let Some(frame) = decoder.decode(&mut buf) {
            dispatch(frame, &state, &incoming, max_plain);
        }
        tokio::select! {
            () = stop.cancelled() => break None,
            read = rd.read_buf(&mut buf) => match read {
                Ok(0) => break None,
                Ok(_) => {}
                Err(e) => break Some(e.into()),
            },
        }
    };
    stop.cancel();
    let pending: Vec<Reply> = {
        let mut st = lock(&state);
        st.closed = true;
        st.pending.drain().map(|(_, (_, tx))| tx).collect()
    };
    for tx in pending {
        let _ = tx.send(Err(TransportError::Closed));
    }
    let _ = incoming.send(Incoming::Closed(outcome));
}

fn dispatch(
    frame: Frame,
    state: &Mutex<State>,
    incoming: &mpsc::UnboundedSender<Incoming>,
    max_plain: usize,
) {
    let header = frame.header;
    let cmd = header.cmd();
    let plain = frame
        .plain_payload(max_plain)
        .map(|p| Bytes::copy_from_slice(&p));
    let is_answer = matches!(cmd, Cmd::Response | Cmd::Error);
    let waiting = if is_answer {
        lock(state).pending.remove(&header.seq)
    } else {
        None
    };
    match waiting {
        Some((opcode, tx)) => {
            let result = match plain {
                Err(e) => Err(e.into()),
                Ok(_) if header.opcode != opcode => Err(TransportError::OpcodeMismatch {
                    expected: opcode,
                    got: header.opcode,
                }),
                Ok(payload) if cmd == Cmd::Error => Err(TransportError::Server {
                    opcode,
                    payload: decode_value(&payload).unwrap_or(rmpv::Value::Nil),
                }),
                Ok(payload) => Ok(Response { header, payload }),
            };
            let _ = tx.send(result);
        }
        None if is_answer => {
            debug!(
                seq = header.seq,
                opcode = header.opcode,
                "late or unknown response dropped"
            );
        }
        None => match plain {
            Ok(payload) => {
                let _ = incoming.send(Incoming::Push(Response { header, payload }));
            }
            Err(e) => warn!(opcode = header.opcode, error = %e, "push frame dropped"),
        },
    }
}

async fn pinger(inner: Weak<Inner>, every: Duration, interactive: bool, stop: CancellationToken) {
    let mut tick = tokio::time::interval_at(Instant::now() + every, every);
    tick.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let payload = match to_msgpack(&Ping { interactive }) {
        Ok(p) => Bytes::from(p),
        Err(e) => {
            warn!(error = %e, "ping payload");
            return;
        }
    };
    loop {
        tokio::select! {
            () = stop.cancelled() => return,
            _ = tick.tick() => {}
        }
        let Some(inner) = inner.upgrade() else { return };
        let transport = Transport { inner };
        match transport
            .request(Opcode::Ping.as_u16(), payload.clone())
            .await
        {
            Ok(_) => {}
            Err(TransportError::Timeout { .. }) => {
                warn!("ping timed out; closing the connection");
                transport.close();
                return;
            }
            Err(_) => return,
        }
    }
}
