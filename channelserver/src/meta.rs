use std::collections::BTreeMap;

use actix::Addr;
use actix_web::{http, HttpRequest};
use maxminddb::{self, geoip2::City, MaxMindDBError};

use logging;
use session::WsChannelSessionState;

// Sender meta data, drawn from the HTTP Headers of the connection counterpart.
#[derive(Serialize, Debug, Default, Clone)]
pub struct SenderData {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ua: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub remote: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub city: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub region: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub country: Option<String>,
}

// Parse the Accept-Language header to get the list of preferred languages.
// We default to "en" because of well-established Anglo-biases.
fn preferred_languages(alheader: String) -> Vec<String> {
    let default_lang = String::from("en");
    let mut lang_tree: BTreeMap<String, String> = BTreeMap::new();
    let mut i = 0;
    alheader.split(",").for_each(|l| {
        if l.contains(";") {
            let weight: Vec<&str> = l.split(";").collect();
            let lang = weight[0].to_ascii_lowercase();
            let pref = weight[1].to_ascii_lowercase();
            lang_tree.insert(String::from(pref.trim()), String::from(lang.trim()));
        } else {
            lang_tree.insert(
                format!("q=1.{:02}", i),
                String::from(l.to_ascii_lowercase()),
            );
            i += 1;
        }
    });
    let mut langs: Vec<String> = lang_tree.values().map(|l| l.to_owned()).collect();
    langs.reverse();
    langs.push(default_lang);
    langs
}

// Return the element that most closely matches the preferred language.
// This rounds up from the dialect if possible.
fn get_preferred_language_element(
    langs: &Vec<String>,
    elements: BTreeMap<String, String>,
) -> Option<String> {
    for lang in langs {
        // It's a wildcard, so just return the first possible choice.
        if lang == "*" {
            return elements.values().into_iter().next().map(|e| e.to_owned());
        }
        if elements.contains_key(lang) {
            if let Some(element) = elements.get(lang.as_str()) {
                return Some(element.to_string());
            }
        }
        if lang.contains("-") {
            let (lang, _) = lang.split_at(2);
            if elements.contains_key(lang) {
                if let Some(element) = elements.get(lang) {
                    return Some(element.to_string());
                }
            }
        }
    }
    None
}

fn handle_city_err(log: Option<&Addr<logging::MozLogger>>, err: &MaxMindDBError) {
    match err {
        maxminddb::MaxMindDBError::InvalidDatabaseError(s) => log.map(|l| {
            l.do_send(logging::LogMessage {
                level: logging::ErrorLevel::Critical,
                msg: format!("Invalid GeoIP database! {:?}", s),
            })
        }),
        maxminddb::MaxMindDBError::IoError(s) => log.map(|l| {
            l.do_send(logging::LogMessage {
                level: logging::ErrorLevel::Critical,
                msg: format!("Could not read from database file! {:?}", s),
            })
        }),
        maxminddb::MaxMindDBError::MapError(s) => log.map(|l| {
            l.do_send(logging::LogMessage {
                level: logging::ErrorLevel::Warn,
                msg: format!("Mapping error: {:?}", s),
            })
        }),
        maxminddb::MaxMindDBError::DecodingError(s) => log.map(|l| {
            l.do_send(logging::LogMessage {
                level: logging::ErrorLevel::Warn,
                msg: format!("Could not decode mapping result: {:?}", s),
            })
        }),
        maxminddb::MaxMindDBError::AddressNotFoundError(s) => log.map(|l| {
            l.do_send(logging::LogMessage {
                level: logging::ErrorLevel::Debug,
                msg: format!("Could not find address for IP: {:?}", s),
            })
        }),
        _ => log.map(|l| {
            l.do_send(logging::LogMessage {
                level: logging::ErrorLevel::Error,
                msg: format!("Unknown GeoIP error encountered: {:?}", err),
            })
        }),
    };
}

fn get_ua(headers: &http::HeaderMap, log: Option<&Addr<logging::MozLogger>>) -> Option<String> {
    if let Some(ua) = headers
        .get(http::header::USER_AGENT)
        .map(|s| match s.to_str() {
            Err(x) => {
                log.map(|l| {
                    l.do_send(logging::LogMessage {
                        level: logging::ErrorLevel::Warn,
                        msg: format!("Bad UA string: {:?}", x),
                    })
                });
                // We have to return Some value here.
                return "".to_owned();
            }
            Ok(s) => s.to_owned(),
        }) {
        if ua == "".to_owned() {
            // If it's blank, it's None.
            return None;
        }
        return Some(ua);
    }
    None
}

fn get_location(
    sender: &mut SenderData,
    langs: &Vec<String>,
    log: Option<&Addr<logging::MozLogger>>,
    iploc: &maxminddb::Reader,
) {
    if sender.remote.is_some() {
        log.map(|l| {
            l.do_send(logging::LogMessage {
                level: logging::ErrorLevel::Debug,
                msg: format!("Looking up IP: {:?}", sender.remote),
            })
        });
        // Strip the port from the remote (if present)
        let remote = sender
            .remote
            .clone()
            .map(|mut r| {
                let end = r.find(':').unwrap_or(r.len());
                r.drain(..end).collect()
            })
            .unwrap_or(String::from(""));
        if let Ok(loc) = remote.parse() {
            if let Ok(city) = iploc.lookup::<City>(loc).map_err(|err| {
                handle_city_err(log, &err);
                err
            }) {
                /*
                    The structure of the returned maxminddb record is:
                    City:maxminddb::geoip::model::City {
                        city: Some(City{
                            geoname_id: Some(#),
                            names: Some({"lang": "name", ...})
                            }),
                        continent: Some(Continent{
                            geoname_id: Some(#),
                            names: Some({...})
                            }),
                        country: Some(Country{
                            geoname_id: Some(#),
                            names: Some({...})
                            }),
                        location: Some(Location{
                            latitude: Some(#.#),
                            longitude: Some(#.#),
                            metro_code: Some(#),
                            time_zone: Some(".."),
                            }),
                        postal: Some(Postal {
                            code: Some("..")
                            }),
                        registered_country: Some(Country {
                            geoname_id: Some(#),
                            iso_code: Some(".."),
                            names: Some({"lang": "name", ...})
                            }),
                        represented_country: None,
                        subdivisions: Some([Subdivision {
                            geoname_id: Some(#),
                            iso_code: Some(".."),
                            names: Some({"lang": "name", ...})
                            }]),
                        traits: None }
                    }
                */
                if let Some(names) = city
                    .city
                    .and_then(|c: maxminddb::geoip2::model::City| c.names)
                {
                    sender.city = get_preferred_language_element(&langs, names);
                }
                if let Some(names) = city
                    .country
                    .and_then(|c: maxminddb::geoip2::model::Country| c.names)
                {
                    sender.country = get_preferred_language_element(&langs, names);
                }
                // because consistency is overrated.
                for subdivision in city.subdivisions {
                    if let Some(subdivision) = subdivision.get(0) {
                        if let Some(names) = subdivision.clone().names {
                            sender.region = get_preferred_language_element(&langs, names);
                            break;
                        }
                    }
                }
            } else {
                log.map(|l| {
                    l.do_send(logging::LogMessage {
                        level: logging::ErrorLevel::Info,
                        msg: format!("No location info for IP: {:?}", sender.remote),
                    })
                });
            }
        }
    }
}

// Set the sender meta information from the request headers.
impl From<HttpRequest<WsChannelSessionState>> for SenderData {
    fn from(req: HttpRequest<WsChannelSessionState>) -> Self {
        let mut sender = SenderData::default();
        let headers = req.headers();
        let log = req.state().log.clone();
        let langs = match headers.get(http::header::ACCEPT_LANGUAGE) {
            None => vec![String::from("*")],
            Some(l) => {
                let lang = match l.to_str() {
                    Err(err) => {
                        log.do_send(logging::LogMessage {
                            level: logging::ErrorLevel::Warn,
                            msg: format!("Bad Accept-Language string: {:?}", err),
                        });
                        "*"
                    }
                    Ok(ls) => ls,
                };
                preferred_languages(lang.to_owned())
            }
        };
        // parse user-header for platform info
        sender.ua = get_ua(&headers, Some(&log));
        sender.remote = req.connection_info().remote().map(|rem| rem.to_owned());
        get_location(&mut sender, &langs, Some(&log), &req.state().iploc);
        sender
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use std::collections::BTreeMap;
    
    use http;

    #[test]
    fn test_preferred_language() {
        let langs = preferred_languages("en-US,es;q=0.1,en;q=0.5,*;q=0.2".to_owned());
        assert_eq!(
            vec![
                "en-us".to_owned(),
                "en".to_owned(),
                "*".to_owned(),
                "es".to_owned(),
                "en".to_owned(),
            ],
            langs
        );
    }

    #[test]
    fn test_get_preferred_language_element() {
        let langs = vec![
            "en-us".to_owned(),
            "en".to_owned(),
            "es".to_owned(),
            "en".to_owned(),
        ];
        // Don't include the default "en" so we can test no matching languages.
        let bad_lang = vec!["fu".to_owned()];
        // Include the "*" so we can return any language.
        let any_lang = vec!["fu".to_owned(), "*".to_owned(), "en".to_owned()];
        let mut elements = BTreeMap::new();
        elements.insert("de".to_owned(), "Kalifornien".to_owned());
        elements.insert("en".to_owned(), "California".to_owned());
        elements.insert("fr".to_owned(), "Californie".to_owned());
        elements.insert("ja".to_owned(), "カリフォルニア州".to_owned());
        assert_eq!(
            Some("California".to_owned()),
            get_preferred_language_element(&langs, elements.clone())
        );
        assert_eq!(
            None,
            get_preferred_language_element(&bad_lang, elements.clone())
        );
        // Return Dutch, since it's the first key listed.
        assert!(get_preferred_language_element(&any_lang, elements.clone()).is_some());
        let goof_lang = vec!["🙄💩".to_owned()];
        assert_eq!(
            None,
            get_preferred_language_element(&goof_lang, elements.clone())
        );
    }

    #[test]
    fn test_ua() {
        let good_header = "Mozilla/5.0 Foo";
        let blank_header = "";
        let mut good_headers = http::HeaderMap::new();
        good_headers.insert(http::header::USER_AGENT, http::header::HeaderValue::from_static(good_header));
        assert_eq!(Some(good_header.to_owned()), get_ua(&good_headers, None));
        let mut blank_headers = http::HeaderMap::new();
        blank_headers.insert(http::header::USER_AGENT, http::header::HeaderValue::from_static(blank_header));
        assert_eq!(None, get_ua(&blank_headers, None));
        let empty_headers = http::HeaderMap::new();
        assert_eq!(None, get_ua(&empty_headers, None));
    }

    #[ignore]
    #[test]
    fn test_location() {
        let test_ip = "63.245.208.195"; // Mozilla

        let langs = vec!["en".to_owned()];
        let mut sender = SenderData::default();
        sender.remote = Some(test_ip.to_owned());
        // TODO: either mock maxminddb::Reader or pass it in as a wrapped impl
    }
}
