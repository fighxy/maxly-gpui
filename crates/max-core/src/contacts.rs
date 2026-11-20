//! Contact details for ids the client has no name for (`CONTACT_INFO`, opcode 32).
//!
//! `LOGIN` with `contactsSync` 0 answered without a `contacts` list in a live run, so dialog
//! peers and group senders arrive as bare ids. The request sends only `contactIds` (as PyMax
//! does); the desktop layout also has `contactType` and `chatId`, whose values are not decoded.

use max_proto::Opcode;
use max_proto::schema::packets::contact::contact_info;
use max_transport::Transport;
use serde::Serialize;

use crate::Result;

/// A contact (`Api::OneMe::Types::Contact`).
pub type Contact = max_proto::schema::types::Contact;

#[derive(Serialize)]
struct Params<'a> {
    #[serde(rename = "contactIds")]
    contact_ids: &'a [i64],
}

/// Ask for the contacts with these ids.
pub async fn contact_info(transport: &Transport, ids: &[i64]) -> Result<Vec<Contact>> {
    let r: contact_info::Response = transport
        .call(Opcode::ContactInfo, &Params { contact_ids: ids })
        .await?;
    Ok(r.contacts)
}
