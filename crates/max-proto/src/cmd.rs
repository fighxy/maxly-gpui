/// Frame kind, the `cmd` byte of the header.
///
/// `Request` (0) and `Response` (1) are confirmed by the capture `cap1`; `Event` (2) and
/// `Error` (3) come from PyMax and are still to be confirmed against the desktop client.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Cmd {
    /// Client request.
    Request,
    /// Server response to a request with the same `seq`.
    Response,
    /// Server push event (unconfirmed value).
    Event,
    /// Server error response (unconfirmed value).
    Error,
    /// Any other value, kept as is.
    Other(u8),
}

impl Cmd {
    /// The raw header byte.
    pub const fn as_u8(self) -> u8 {
        match self {
            Cmd::Request => 0,
            Cmd::Response => 1,
            Cmd::Event => 2,
            Cmd::Error => 3,
            Cmd::Other(v) => v,
        }
    }
}

impl From<u8> for Cmd {
    fn from(value: u8) -> Self {
        match value {
            0 => Cmd::Request,
            1 => Cmd::Response,
            2 => Cmd::Event,
            3 => Cmd::Error,
            v => Cmd::Other(v),
        }
    }
}

impl From<Cmd> for u8 {
    fn from(cmd: Cmd) -> u8 {
        cmd.as_u8()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn roundtrip_every_byte() {
        for v in 0..=u8::MAX {
            assert_eq!(Cmd::from(v).as_u8(), v);
        }
    }
}
