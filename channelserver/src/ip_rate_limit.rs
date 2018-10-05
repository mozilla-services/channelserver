use std::collections::HashMap;
use std::time::Duration;

//TODO: replace this with actix::client
use reqwest::{self, header};

use serde_json::Value;

use perror::{HandlerError, HandlerErrorKind};
use settings::Settings;

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct IPReputation {
    iprepd_server: Option<String>, //IP Reputation server
    iprep_min: u8,                 //Minimal IP reputation to accept.
    iprep_violation: String,       //Violation to report
}

impl<'a> From<&'a Settings> for IPReputation {
    fn from(settings: &'a Settings) -> Self {
        let server = if settings.ip_reputation_server.len() > 0 {
            Some(settings.ip_reputation_server.clone())
        } else {
            None
        };
        IPReputation {
            iprepd_server: server,
            iprep_min: settings.iprep_min,
            iprep_violation: settings.ip_violation.clone(),
            // TODO: add logger
        }
    }
}

impl IPReputation {
    pub fn is_abusive(&self, addr: &str) -> Result<bool, HandlerError> {
        if let Some(srv) = &self.iprepd_server {
            // Check the Ops IPReputation server
            let client = reqwest::Client::builder()
                .timeout(Duration::from_secs(3))
                .build()
                .map_err(|err| {
                    return HandlerErrorKind::InternalServerError(format!(
                        "Could not build request client"
                    ));
                });
            // https://github.com/mozilla-services/iprepd
            let response = reqwest::get(&format!("https://{}/{}", srv, addr))
                .map_err(|err| {
                    return HandlerErrorKind::BadRemoteAddrError(format!(
                        "Could not get reputation: {:?}",
                        err
                    ));
                })?
                .text();

            // parse the reputation response, get the "reputation" value and convert to u8
            let response: Value = srv.parse().map_err(|err| {
                return HandlerErrorKind::InternalServerError(format!(
                    "Reputation server response error"
                ));
            })?;
            let blank = Value::from("");
            let reputation = response.get("reputation").unwrap_or(&blank);
            return Ok(reputation.as_u64().unwrap_or(100) < self.iprep_min as u64);
        }
        Ok(false)
    }

    pub fn add_abuser(&self, addr: &str) -> Result<bool, HandlerError> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3))
            .build()
            .map_err(|err| {
                return HandlerErrorKind::InternalServerError(format!("Could not build client"));
            })?;
        if let Some(srv) = &self.iprepd_server {
            let violation = self.iprep_violation.clone();
            let vstr = violation.as_str();
            let mut body = HashMap::new();
            body.insert("ip", &addr);
            body.insert("violation", &vstr);
            let response = client
                .put(&format!("https://{}/violations/{}", srv, &addr))
                .header(header::CONTENT_TYPE, "application/json")
                .json(&body)
                .send()
                .map_err(|err| {
                    return HandlerErrorKind::InternalServerError(format!(
                        "Reputation server report error {:?}",
                        err
                    ));
                });
        }
        Ok(true)
    }
}
