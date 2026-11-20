//! Account sessions and logout (`SESSIONS_INFO` 96, `SESSIONS_CLOSE` 97, `LOGOUT` 20).
//!
//! All three requests have no parameters in `core.dll` (no `Parameters` layout) and are sent as
//! an empty map, as PyMax does. `SESSIONS_CLOSE` ends every other session and answers with a new
//! login token for this one; [`crate::SupervisorHandle::close_other_sessions`] stores it.

use max_proto::Opcode;
use max_proto::schema::packets::sessions_info;
use max_transport::Transport;
use serde::Serialize;

use crate::Result;

/// One logged-in device (`Api::OneMe::Types::Session`).
pub type SessionInfo = max_proto::schema::types::Session;

/// The empty parameter map these requests carry.
#[derive(Serialize)]
pub(crate) struct NoParams {}

/// The account's active sessions, this one included (`current`).
pub async fn sessions(transport: &Transport) -> Result<Vec<SessionInfo>> {
    let r: sessions_info::Response = transport.call(Opcode::SessionsInfo, &NoParams {}).await?;
    Ok(r.sessions)
}
