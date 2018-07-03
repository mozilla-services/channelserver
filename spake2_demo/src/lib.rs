#![feature(proc_macro)]

#[macro_use]
extern crate stdweb;
#[macro_use]
extern crate serde_derive;
#[macro_use]
extern crate lazy_static;
extern crate serde_json;
extern crate spake2;
extern crate hex;

use stdweb::js_export;
use spake2::{Ed25519Group, SPAKE2};

use std::collections::HashMap;
use std::sync::Mutex;

lazy_static! {
  static ref SPAKE_MAP: Mutex<HashMap<String, SPAKE2<Ed25519Group>>> = Mutex::new(HashMap::new());
}

#[js_export]
fn start(code: &str, id: &str) -> String {
  let spake_map: &mut HashMap<String, SPAKE2<Ed25519Group>> = &mut SPAKE_MAP.lock().unwrap();
  if spake_map.contains_key(code) { panic!("code already in use"); }
  let (s, msg) = SPAKE2::<Ed25519Group>::start_symmetric(
    code.as_bytes(),
    id.as_bytes()
  );
  spake_map.insert(String::from(code), s);
  hex::encode(msg)
}

#[js_export]
fn finish(code: &str, msg: &str) -> String {
  let spake_map: &mut HashMap<String, SPAKE2<Ed25519Group>> = &mut SPAKE_MAP.lock().unwrap();
  match spake_map.remove(code) {
    None => panic!("code not in use"),
    Some(s) => {
      let msg_bytes = match hex::decode(msg) {
        Err(_) => panic!("hex decode error"),
        Ok(b) => b
      };
      match s.finish(msg_bytes.as_slice()) {
        Ok(key) => hex::encode(key),
        Err(_) => panic!("SPAKE2 error")
      }
    }
  }
}

#[js_export]
fn abort(_code: &str) {
  ()
}

#[derive(Serialize)]
struct OneShotResult {
  key: String,
  msg: String
}

js_serializable!( OneShotResult );

#[js_export]
fn oneshot(code: &str, id: &str, msg: &str) -> OneShotResult {
  let (s, reply) = SPAKE2::<Ed25519Group>::start_symmetric(
    code.as_bytes(),
    id.as_bytes()
  );
  let msg_bytes = match hex::decode(msg) {
    Err(_) => panic!("hex decode error"),
    Ok(b) => b
  };
  match s.finish(msg_bytes.as_slice()) {
    Err(_) => panic!("SPAKE2 error"),
    Ok(key) => OneShotResult{ key: hex::encode(key), msg: hex::encode(reply) }
  }
}
