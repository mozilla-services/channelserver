use std::collections::BTreeMap;

use actix::Addr;
use actix_web::{http, HttpRequest};
use http::header::{self, HeaderName};
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

fn get_remote(headers: &http::HeaderMap, allowlist: &[String]) -> Option<String> {
    // Actix determines the connection_info.remote() from the first entry in the
    // Forwarded then X-Fowarded-For then peer name. The problem is that any
    // of those could be multiple entries or may point to a known proxy.
    // Check the [FORWARDED](https://developer.mozilla.org/en-US/docs/Web/HTTP/Headers/Forwarded) 
    // header first.
    // Forwarded is a comma separated set of sub-fields that indicate the prior connection. The
    // `for=` sub key identifies the origin. Each new server should be appended to 
    // the end of this list. 
    // (e.g. 
    // `Forwarded: by=<identifier>; for=<identifier>; host=<host>; proto=<http|https>, for=<identifier>...` 
    // ) 
    for header in headers.get_all(header::FORWARDED) {
        if let Ok(val) = header.to_str() {
            // Remember, latest servers are appended. See 
            // https://tools.ietf.org/html/rfc7239#section-4
            let mut components:Vec<&str> = val.split(',').collect();
            components.reverse();
            for set in components {
                for el in set.split(';') {
                    let mut items = el.trim().splitn(2, '=');
                    if let Some(name) = items.next() {
                        if let Some(val) = items.next() {
                            // there are four qualified identifiers:
                            // by: the interface where the request came in.
                            // for: the client that initiated the request
                            // host: the Host request header as rec'vd by the proxy
                            // proto: the protocol.
                            // "for" is analagous to the value from X-Forwarded-For, but
                            // some argument could be made for using "host"
                            if &name.to_lowercase() as &str == "for" && !allowlist.contains(&val.trim().to_owned()) {
                                return Some(val.trim().to_owned());
                            }
                        }
                    }
                }
            }
        }
    }
    // And then the backup headers
    for backups in vec!["x-forwarded-for"] {
        let backup = backups.as_bytes();
        if let Some(header) = headers.get(HeaderName::from_lowercase(backup).unwrap()) {
            if let Ok(hstr) = header.to_str() {
                // Just like `Forward` successive proxies are appeneded to this header.
                let mut host_list:Vec<&str> = hstr.split(',').collect();
                host_list.reverse();
                for host_str in host_list {
                    let host = host_str.trim().to_owned();
                    if !allowlist.contains(&host) {
                        return Some(host);
                    }
                }
            }
        }
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
        // Ideally, this would just get &req. For testing, I'm passing in the values.
        sender.remote = get_remote(&req.headers(), &req.state().proxy_allowlist);
        get_location(&mut sender, &langs, Some(&log), &req.state().iploc);
        sender
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use actix_web::{self, server, HttpRequest};
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
        good_headers.insert(
            http::header::USER_AGENT,
            http::header::HeaderValue::from_static(good_header),
        );
        assert_eq!(Some(good_header.to_owned()), get_ua(&good_headers, None));
        let mut blank_headers = http::HeaderMap::new();
        blank_headers.insert(
            http::header::USER_AGENT,
            http::header::HeaderValue::from_static(blank_header),
        );
        assert_eq!(None, get_ua(&blank_headers, None));
        let empty_headers = http::HeaderMap::new();
        assert_eq!(None, get_ua(&empty_headers, None));
    }

    #[test]
    fn test_location_good() {
        let test_ip = "63.245.208.195"; // Mozilla

        let langs = vec!["en".to_owned()];
        let mut sender = SenderData::default();
        sender.remote = Some(test_ip.to_owned());
        // TODO: either mock maxminddb::Reader or pass it in as a wrapped impl
        let iploc = maxminddb::Reader::open("mmdb/latest/GeoLite2-City.mmdb").unwrap();
        get_location(&mut sender, &langs, None, &iploc);
        assert_eq!(sender.city, Some("Sacramento".to_owned()));
        assert_eq!(sender.region, Some("California".to_owned()));
        assert_eq!(sender.country, Some("United States".to_owned()));
    }

    #[test]
    fn test_location_bad() {
        let test_ip = "192.168.1.1";

        let langs = vec!["en".to_owned()];
        let mut sender = SenderData::default();
        sender.remote = Some(test_ip.to_owned());
        // TODO: either mock maxminddb::Reader or pass it in as a wrapped impl
        let iploc = maxminddb::Reader::open("mmdb/latest/GeoLite2-City.mmdb").unwrap();
        get_location(&mut sender, &langs, None, &iploc);
        assert_eq!(sender.city, None);
        assert_eq!(sender.region, None);
        assert_eq!(sender.country, None);
    }

    #[test]
    fn test_get_remote() {
        let mut headers = actix_web::http::header::HeaderMap::new();

        let allowlist = vec!["192.168.0.1".to_owned()];

        headers.insert(
            http::header::HeaderName::from_lowercase("x-forwarded-for".as_bytes()).unwrap(),
            "10.10.10.10, 192.168.0.1".parse().unwrap(),
        );

        let remote = get_remote(&headers, &allowlist);
        assert_eq!(remote, Some("10.10.10.10".to_owned()));

        // Adding a header which should override the previous "success"
        headers.insert(
            http::header::HeaderName::from_lowercase("x-forwarded-for".as_bytes()).unwrap(),
            "10.11.11.11, 192.168.0.1".parse().unwrap(),
        );

        let remote = get_remote(&headers, &allowlist);
        assert_eq!(remote, Some("10.11.11.11".to_owned()));

        // Adding the Primary header
        headers.insert(http::header::HeaderName::from_lowercase("forwarded".as_bytes()).unwrap(),
            "by=10.10.10.10;proto=http;for=10.12.12.12;host=10.13.13.13,for=192.168.0.1;by=10.09.09.09".parse().unwrap());

        let remote = get_remote(&headers, &allowlist);
        assert_eq!(remote, Some("10.12.12.12".to_owned()));
    }
}
