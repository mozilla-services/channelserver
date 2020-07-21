use std::fmt;

use rand::RngCore;
use serde::ser::{Serialize, Serializer};

const CHANNELID_LEN: usize = 16;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct ChannelID {
    value: [u8; CHANNELID_LEN],
}

impl ChannelID {
    pub fn as_string(self) -> String {
        base64::encode_config(&self.value, base64::URL_SAFE_NO_PAD)
    }

    pub fn from_str(string: &str) -> Result<ChannelID, base64::DecodeError> {
        let bytes = base64::decode_config(string, base64::URL_SAFE_NO_PAD)?;
        let mut array = [0; 16];
        array.copy_from_slice(&bytes[..16]);
        Ok(ChannelID { value: array })
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
        serializer.serialize_str(&self.as_string())
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_parse() {
        let raw_id = "j6jLPVPeQR6diyrkQinRAQ";
        // From URLSafe b64
        let chan = ChannelID::from_str(raw_id).unwrap();
        assert!(chan.as_string() == raw_id.to_owned());
        ChannelID::from_str("invalid").expect_err("rejected");
        let output = format!("{}", chan);
        assert_eq!("j6jLPVPeQR6diyrkQinRAQ".to_owned(), output);
    }
}
