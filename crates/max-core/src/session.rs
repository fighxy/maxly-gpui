use max_identity::{Environment, session_init};
use max_proto::Opcode;
use max_proto::packets::{
    AuthByTrack, AuthByTrackResponse, AuthCheck, AuthCheckResponse, CreateQr, CreateQrResponse,
    SessionInitResponse,
};
use max_proto::schema::packets::auth;
use max_proto::schema::packets::auth::one_me::PasswordChallenge;
use max_store::Profile;
use max_transport::{Incoming, Transport, connect, tls_config_for};
use tokio::io::{AsyncRead, AsyncWrite};
use tokio::sync::mpsc::UnboundedReceiver;
use zeroize::Zeroizing;

use crate::auth::{
    AuthStep, CHECK_CODE, CodeRequest, QrLogin, QrPoll, RESEND, START_AUTH, auth_step,
};
use crate::{CoreConfig, CoreError, Result, SyncState};

/// `LOGIN` response (`Api::OneMe::Packets::Auth::Login::Response`): profile, chats, contacts,
/// presence, configuration, recent messages.
pub type LoginResponse = auth::login::Response;

/// One connection after `SESSION_INIT`.
pub struct Session {
    transport: Transport,
    pushes: UnboundedReceiver<Incoming>,
    init: SessionInitResponse,
}

impl Session {
    /// Connect over TLS to the configured endpoint and send `SESSION_INIT`.
    pub async fn connect(
        config: &CoreConfig,
        profile: &mut Profile,
        env: &Environment,
    ) -> Result<Session> {
        let stream = connect(
            &config.endpoint,
            tls_config_for(&config.endpoint.host)?,
            config.connect_timeout,
        )
        .await?;
        Session::start(stream, config, profile, env).await
    }

    /// Run a session over any stream (tests, proxies) and send `SESSION_INIT`. Counts a
    /// launch in the profile (`clientSessionId`) before sending.
    pub async fn start<S>(
        stream: S,
        config: &CoreConfig,
        profile: &mut Profile,
        env: &Environment,
    ) -> Result<Session>
    where
        S: AsyncRead + AsyncWrite + Send + 'static,
    {
        let (transport, pushes) = Transport::spawn(stream, config.transport.clone());
        let mut identity = profile.identity().clone();
        let payload = session_init(&config.version, env, &mut identity, &config.identity);
        // Persist the counter the payload carries before it leaves.
        let counted = profile.begin_session()?;
        debug_assert_eq!(Some(counted), payload.client_session_id);
        let init = transport.call(Opcode::SessionInit, &payload).await?;
        Ok(Session {
            transport,
            pushes,
            init,
        })
    }

    /// The server's `SESSION_INIT` answer.
    pub fn init_response(&self) -> &SessionInitResponse {
        &self.init
    }

    /// The connection, for any request.
    pub fn transport(&self) -> &Transport {
        &self.transport
    }

    /// Server pushes and the end of the connection.
    pub fn pushes(&mut self) -> &mut UnboundedReceiver<Incoming> {
        &mut self.pushes
    }

    /// Split into the connection and its push receiver.
    pub fn into_parts(self) -> (Transport, UnboundedReceiver<Incoming>) {
        (self.transport, self.pushes)
    }

    // ---- QR login (B1) --------------------------------------------------------------------

    /// Ask for a QR code (`CREATE_QR`).
    pub async fn start_qr(&self) -> Result<QrLogin> {
        let r: CreateQrResponse = self.transport.call(Opcode::CreateQr, &CreateQr {}).await?;
        match (r.track_id, r.qr_link) {
            (Some(track_id), Some(qr_link)) => Ok(QrLogin {
                track_id,
                qr_link,
                polling_interval_ms: r.polling_interval.unwrap_or(5000),
            }),
            _ => Err(CoreError::UnexpectedAuthAnswer(
                "CREATE_QR without trackId or qrLink".into(),
            )),
        }
    }

    /// Poll the QR state once (`AUTH_CHECK`); call every `polling_interval_ms`.
    pub async fn poll_qr(&self, qr: &QrLogin) -> Result<QrPoll> {
        let r: AuthCheckResponse = self
            .transport
            .call(
                Opcode::AuthCheck,
                &AuthCheck {
                    track_id: qr.track_id.clone(),
                },
            )
            .await?;
        let status = r.status.unwrap_or_default();
        Ok(if status.login_available == Some(true) {
            QrPoll::Ready
        } else {
            QrPoll::Waiting {
                expires_at: status.expires_at,
            }
        })
    }

    /// Finish a QR login after [`QrPoll::Ready`] (`AUTH_BY_TRACK`).
    pub async fn finish_qr(&self, qr: &QrLogin) -> Result<AuthStep> {
        let r: AuthByTrackResponse = self
            .transport
            .call(
                Opcode::AuthByTrack,
                &AuthByTrack {
                    track_id: qr.track_id.clone(),
                },
            )
            .await?;
        auth_step(
            r.token_attrs.as_ref(),
            r.password_challenge.as_ref(),
            r.profile.as_ref(),
        )
    }

    // ---- SMS code login (B2) --------------------------------------------------------------

    /// Ask for an SMS code (`AUTH_REQUEST`, `type` `START_AUTH`, or `RESEND` to repeat).
    pub async fn request_code(
        &self,
        phone: &str,
        language: Option<&str>,
        resend: bool,
    ) -> Result<CodeRequest> {
        let request = auth::one_me::auth_request::Parameters {
            phone: phone.to_string(),
            language: language.map(str::to_string),
            type_: if resend { RESEND } else { START_AUTH }.to_string(),
        };
        let r: auth::one_me::auth_request::Response =
            self.transport.call(Opcode::AuthRequest, &request).await?;
        Ok(CodeRequest {
            token: Zeroizing::new(r.token),
            code_length: r.code_length,
            // Sent in milliseconds (60000 in a live answer); kept in whole seconds.
            alt_action_after: (r.alt_action_duration + 999) / 1000,
            requests_left: r.request_count_left,
        })
    }

    /// Send the SMS code (`AUTH`, `authTokenType` `CHECK_CODE`).
    pub async fn confirm_code(&self, request: &CodeRequest, code: &str) -> Result<AuthStep> {
        let params = auth::one_me::auth::Parameters {
            token: request.token.to_string(),
            verify_code: code.to_string(),
            auth_token_type: CHECK_CODE.to_string(),
        };
        let r: auth::one_me::auth::Response = self.transport.call(Opcode::Auth, &params).await?;
        auth_step(
            Some(&r.token_attrs),
            r.password_challenge.as_ref(),
            r.profile.as_ref(),
        )
    }

    // ---- 2FA password (B3) ----------------------------------------------------------------

    /// Send the 2FA password for a challenge (`AUTH_LOGIN_CHECK_PASSWORD`).
    pub async fn check_password(
        &self,
        challenge: &PasswordChallenge,
        password: &str,
    ) -> Result<AuthStep> {
        let params = auth::one_me::login_check_password::Parameters {
            password: password.to_string(),
            track_id: challenge.track_id.clone(),
        };
        let r: auth::one_me::login_check_password::Response = self
            .transport
            .call(Opcode::LoginCheckPassword, &params)
            .await?;
        auth_step(Some(&r.token_attrs), None, r.profile.as_ref())
    }

    // ---- LOGIN (A5) -----------------------------------------------------------------------

    /// Log in with a token (`LOGIN`) and receive the initial sync.
    pub async fn login(
        &self,
        token: &str,
        sync: &SyncState,
        interactive: bool,
    ) -> Result<LoginResponse> {
        let params = auth::login::Parameters {
            token: token.to_string(),
            chat_cache_fingerprint: None,
            chats_sync: sync.chats,
            contacts_sync: sync.contacts,
            presence_sync: sync.presence,
            calls_sync: sync.calls,
            drafts_sync: sync.drafts,
            banners_sync: None,
            config_hash: sync.config_hash.clone(),
            interactive,
            last_login: sync.last_login,
        };
        Ok(self.transport.call(Opcode::Login, &params).await?)
    }

    /// Keep the token of a finished login step in the profile and send `LOGIN`. Returns
    /// `None` when the step needs more input (password, registration).
    pub async fn complete(
        &self,
        profile: &Profile,
        step: &AuthStep,
        sync: &SyncState,
    ) -> Result<Option<LoginResponse>> {
        match step {
            AuthStep::LoggedIn { token, .. } => {
                profile.set_auth_token(token)?;
                Ok(Some(self.login(token, sync, true).await?))
            }
            AuthStep::PasswordRequired(_) | AuthStep::RegistrationRequired => Ok(None),
        }
    }
}
