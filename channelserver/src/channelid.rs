use std::fmt;

use base64;
use rand::RngCore;
use serde::ser::{Serialize, Serializer};

const CHANNELID_LEN: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ChannelID {
    value: [u8; CHANNELID_LEN],
}

impl ChannelID {
    pub fn is_valid(self) -> bool {
        !self.value.iter().all(|&b| b == 0)
    }

    pub fn nil() -> Self {
        Self {
            value: [0; CHANNELID_LEN],
        }
    }

    pub fn to_string(self) -> String {
        base64::encode_config(&self.value, base64::URL_SAFE_NO_PAD)
    }
}

impl Default for ChannelID {
    fn default() -> Self {
        let mut rng = rand::thread_rng();
        let mut bytes = [0; CHANNELID_LEN];
        rng.fill_bytes(&mut bytes);
        Self { value: bytes }
    }
}

impl fmt::Display for ChannelID {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // calling to_string() causes a stack overflow.
        let as_b64 = base64::encode_config(&self.value, base64::URL_SAFE_NO_PAD);
        write!(f, "{}", as_b64)
    }
}

impl Serialize for ChannelID {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.serialize_str(&self.to_string())
    }
}

impl<'a> From<&'a str> for ChannelID {
    fn from(string: &str) -> ChannelID {
        let bytes = match base64::decode_config(string, base64::URL_SAFE_NO_PAD) {
            Ok(b) => b,
            Err(err) => {
                return ChannelID::nil();
            }
        };
        let mut array = [0; 16];
        array.copy_from_slice(&bytes[..16]);
        ChannelID { value: array }
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_core() {
        let channel_nil = ChannelID::nil();
        assert!(!channel_nil.is_valid());
        let channel_valid = ChannelID::default();
        assert!(channel_valid.is_valid());
    }

    #[test]
    fn test_parse() {
        let raw_id = "j6jLPVPeQR6diyrkQinRAQ";
        // From URLSafe b64
        let chan = ChannelID::from(raw_id);
        assert!(chan.is_valid());
        assert!(chan.to_string() == raw_id.to_owned());
        let bad_chan = ChannelID::from("invalid");
        assert!(!bad_chan.is_valid());
        let output = format!("{}", chan);
        assert_eq!("j6jLPVPeQR6diyrkQinRAQ".to_owned(), output);
    }
}
