use std::fmt;

use base64;
use serde::ser::{Serialize, Serializer};
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ChannelID {
    value: Uuid,
}

impl ChannelID {
    pub fn is_valid(self) -> bool {
        &self.value != &Uuid::nil()
    }

    pub fn to_string(self) -> String {
        base64::encode_config(&self.value.as_bytes(), base64::URL_SAFE_NO_PAD)
    }
}

impl Default for ChannelID {
    fn default() -> Self {
        Self {
            value: Uuid::new_v4(),
        }
    }
}

impl fmt::Display for ChannelID {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        // calling to_string() causes a stack overflow.
        let as_b64 = base64::encode_config(self.value.clone().as_bytes(), base64::URL_SAFE_NO_PAD);
        let as_uuid = self.value.to_string();
        write!(f, "{} [{}]", as_b64, as_uuid)
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
                return ChannelID { value: Uuid::nil() };
            }
        };
        let mut array = [0; 16];
        array.copy_from_slice(&bytes[..16]);
        ChannelID {
            value: Uuid::from_bytes(array),
        }
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use uuid::Uuid;

    #[test]
    fn test_core() {
        let channel_nil = ChannelID { value: Uuid::nil() };
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
        assert_eq!(
            "j6jLPVPeQR6diyrkQinRAQ [8fa8cb3d-53de-411e-9d8b-2ae44229d101]".to_owned(),
            output
        );
    }
}
