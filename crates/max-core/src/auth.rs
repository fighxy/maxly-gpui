use std::collections::BTreeMap;

use max_proto::schema::packets::auth::one_me::{PasswordChallenge, TokenAttributes};
use max_proto::schema::types::Profile;
use zeroize::Zeroizing;

use crate::{CoreError, Result};

/// `AuthRequest.type` for a first code request (`core.dll` `FUN_180814950`).
pub(crate) const START_AUTH: &str = "START_AUTH";
/// `AuthRequest.type` for "send the code again" (`core.dll` `FUN_180813170`).
pub(crate) const RESEND: &str = "RESEND";
/// `Auth.authTokenType` when sending the SMS code (`core.dll` `FUN_1807f9c00`).
pub(crate) const CHECK_CODE: &str = "CHECK_CODE";
/// `tokenAttrs` key of a login token (`core.dll` `FUN_18080f730`).
pub(crate) const TOKEN_LOGIN: &str = "LOGIN";
/// `tokenAttrs` key of a registration token (`core.dll` `FUN_180801a20`).
pub(crate) const TOKEN_REGISTER: &str = "REGISTER";

/// A QR login in progress.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QrLogin {
    /// Flow id for polling and finishing.
    pub track_id: String,
    /// Link to show as a QR code.
    pub qr_link: String,
    /// How often to poll, ms.
    pub polling_interval_ms: i64,
}

/// Result of one QR poll.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QrPoll {
    /// Not scanned yet; the code expires at this Unix time in ms, if the server says.
    Waiting {
        /// Expiry, Unix milliseconds.
        expires_at: Option<i64>,
    },
    /// Scanned and approved on a phone: call [`crate::Session::finish_qr`].
    Ready,
}

/// An SMS code request in progress.
#[derive(Clone)]
pub struct CodeRequest {
    pub(crate) token: Zeroizing<String>,
    /// Number of digits in the code.
    pub code_length: i64,
    /// Seconds until another way to get the code is offered (`altActionDuration`, sent in ms).
    pub alt_action_after: i64,
    /// Requests left, if the server says.
    pub requests_left: Option<i32>,
}

impl std::fmt::Debug for CodeRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CodeRequest")
            .field("code_length", &self.code_length)
            .field("alt_action_after", &self.alt_action_after)
            .field("requests_left", &self.requests_left)
            .finish_non_exhaustive()
    }
}

/// Where a login step leaves the user.
pub enum AuthStep {
    /// Logged in: keep the token and send `LOGIN`.
    LoggedIn {
        /// Login token.
        token: Zeroizing<String>,
        /// Profile, when the server includes it.
        profile: Option<Box<Profile>>,
    },
    /// The account has a 2FA password: ask for it and call
    /// [`crate::Session::check_password`].
    PasswordRequired(Box<PasswordChallenge>),
    /// The phone has no account yet; registration (B4) is not implemented.
    RegistrationRequired,
}

impl std::fmt::Debug for AuthStep {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AuthStep::LoggedIn { profile, .. } => f
                .debug_struct("LoggedIn")
                .field("has_profile", &profile.is_some())
                .finish_non_exhaustive(),
            AuthStep::PasswordRequired(_) => f.write_str("PasswordRequired"),
            AuthStep::RegistrationRequired => f.write_str("RegistrationRequired"),
        }
    }
}

/// Read the outcome of `Auth`, `AuthByTrack` or `LoginCheckPassword`.
pub(crate) fn auth_step(
    token_attrs: Option<&BTreeMap<String, TokenAttributes>>,
    challenge: Option<&PasswordChallenge>,
    profile: Option<&Profile>,
) -> Result<AuthStep> {
    if let Some(login) = token_attrs.and_then(|t| t.get(TOKEN_LOGIN)) {
        if login.token.is_empty() {
            return Err(CoreError::UnexpectedAuthAnswer("empty LOGIN token".into()));
        }
        return Ok(AuthStep::LoggedIn {
            token: Zeroizing::new(login.token.clone()),
            profile: profile.cloned().map(Box::new),
        });
    }
    if let Some(c) = challenge {
        return Ok(AuthStep::PasswordRequired(Box::new(c.clone())));
    }
    if token_attrs.is_some_and(|t| t.contains_key(TOKEN_REGISTER)) {
        return Ok(AuthStep::RegistrationRequired);
    }
    let keys: Vec<&String> = token_attrs.map(|t| t.keys().collect()).unwrap_or_default();
    Err(CoreError::UnexpectedAuthAnswer(format!(
        "no LOGIN token, no password challenge; token kinds {keys:?}"
    )))
}
