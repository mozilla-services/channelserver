use std::collections::{BTreeMap, HashMap};
use std::net::{IpAddr, SocketAddr};

use actix_web::{http, HttpRequest};
use http::header::HeaderName;
use ipnet::IpNet;
use maxminddb::{self, geoip2::City, MaxMindDBError};

use logging;
use perror::{HandlerError, HandlerErrorKind};
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

#[allow(unreachable_patterns)]
fn handle_city_err(log: &logging::MozLogger, err: &MaxMindDBError) {
    match err {
        maxminddb::MaxMindDBError::InvalidDatabaseError(s) => {
            error!(log.log, "Invalid GeoIP database! {:?}", s);
            ::std::process::exit(-1);
        }
        maxminddb::MaxMindDBError::IoError(s) => error!(log.log, "Could not read database {:?}", s),
        maxminddb::MaxMindDBError::MapError(s) => warn!(log.log, "Mapping error: {:?}", s),
        maxminddb::MaxMindDBError::DecodingError(s) => {
            warn!(log.log, "Could not decode mapping result: {:?}", s)
        }
        maxminddb::MaxMindDBError::AddressNotFoundError(s) => {
            debug!(log.log, "Could not find address for IP: {:?}", s)
        }
        // include to future proof against cross compile dependency errors
        _ => error!(log.log, "Unknown GeoIP error encountered: {:?}", err),
    };
}

fn get_ua(
    headers: &http::HeaderMap,
    log: &logging::MozLogger,
    meta: &SenderData,
) -> Option<String> {
    if let Some(ua) = headers
        .get(http::header::USER_AGENT)
        .map(|s| match s.to_str() {
            Err(x) => {
                warn!(
                    log.log,
                    "Bad UA string: {:?}", x;
                    "remote_ip" => &meta.remote
                );
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

fn is_trusted_proxy(proxy_list: &[IpNet], host: &IpAddr) -> bool {
    // Return if an address is part of the allow list
    proxy_list.iter().any(|range| range.contains(host))
}

fn get_remote(
    peer: &Option<SocketAddr>,
    headers: &http::HeaderMap,
    proxy_list: &[IpNet],
    log: &logging::MozLogger,
) -> Result<String, HandlerError> {
    // Actix determines the connection_info.remote() from the first entry in the
    // Forwarded then X-Fowarded-For, Forwarded-For, then peer name. The problem is that any
    // of those could be multiple entries or may point to a known proxy, or be injected by the
    // user. We strictly only check the one header we know the proxy will be sending, working
    // our way back up the proxy chain until we find the first unexpected address.
    // This may be an intermediary proxy, or it may be the original requesting system.
    //
    if peer.is_none() {
        return Err(HandlerErrorKind::BadRemoteAddrError("Peer is unspecified".to_owned()).into());
    }
    let peer_ip = peer.unwrap().ip();
    // if the peer is not a known proxy, ignore the X-Forwarded-For headers
    if !is_trusted_proxy(proxy_list, &peer_ip) {
        return Ok(peer_ip.to_string());
    }

    // The peer is a known proxy, so take rightmost X-Forwarded-For that is not a trusted proxy.
    match headers.get(HeaderName::from_lowercase("x-forwarded-for".as_bytes()).unwrap()) {
        Some(header) => {
            match header.to_str() {
                Ok(hstr) => {
                    info!(log.log, "Remote IP List: {:?}", hstr);
                    // successive proxies are appeneded to this header.
                    let mut host_list: Vec<&str> = hstr.split(',').collect();
                    host_list.reverse();
                    for host_str in host_list {
                        match host_str.trim().parse::<IpAddr>() {
                            Ok(addr) => {
                                if !addr.is_loopback() &&
                                   !is_trusted_proxy(proxy_list, &addr) {
                                    return Ok(addr.to_string());
                                }
                            },
                            Err(err) => {
                                return Err(HandlerErrorKind::BadRemoteAddrError("Bad IP Specified".to_owned()).into());
                            }
                        }
                    };
                    Err(HandlerErrorKind::BadRemoteAddrError("Only proxies specified".to_owned()).into())
                },
                Err(err) => Err(HandlerErrorKind::BadRemoteAddrError(format!(
                    "Unknown address in X-Forwarded-For: {:?}",
                    err
                ))
                .into()),
            }
        }
        None => Err(HandlerErrorKind::BadRemoteAddrError(format!(
            "No X-Forwarded-For found for proxied connection"
        ))
        .into()),
    }
}

fn get_location(
    sender: &mut SenderData,
    langs: &Vec<String>,
    log: &logging::MozLogger,
    iploc: &maxminddb::Reader,
) {
    if sender.remote.is_some() {
        debug!(
            log.log,
            "Looking up IP";
            "remote_ip" => &sender.remote
        );
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
                info!(
                    log.log,
                    "No location info for IP";
                    "remote_ip" => &sender.remote
                )
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
        // Ideally, this would just get &req. For testing, I'm passing in the values.
        sender.remote = match get_remote(
            &req.peer_addr(),
            &req.headers(),
            &req.state().trusted_proxy_list,
            &log,
        ) {
            Ok(addr) => Some(addr),
            Err(err) => {
                error!(
                    log.log,
                    "{:?}", err;
                    "remote_ip" => &sender.remote
                );
                None
            }
        };
        let langs = match headers.get(http::header::ACCEPT_LANGUAGE) {
            None => vec![String::from("*")],
            Some(l) => {
                let lang = match l.to_str() {
                    Err(err) => {
                        warn!(
                            log.log,
                            "Bad Accept-Language string: {:?}", err;
                            "remote_ip" => &sender.remote
                        );
                        "*"
                    }
                    Ok(ls) => ls,
                };
                preferred_languages(lang.to_owned())
            }
        };
        // parse user-header for platform info
        sender.ua = get_ua(&headers, &log, &sender);
        get_location(&mut sender, &langs, &log, &req.state().iploc);
        sender
    }
}

/// Convert the Sender Metadata into a optional hash of data. Only include things that are set.
/// This is used mostly by the logger.
impl Into<Option<HashMap<String, String>>> for SenderData {
    fn into(self) -> Option<HashMap<String, String>> {
        let mut map: HashMap<String, String> = HashMap::new();
        // Do not include UA string for PII reasons.
        if let Some(val) = self.remote {
            map.insert("remote_ip".to_owned(), val);
        }
        if let Some(val) = self.city {
            map.insert("remote_city".to_owned(), val);
        }
        if let Some(val) = self.region {
            map.insert("remote_region".to_owned(), val);
        }
        if let Some(val) = self.country {
            map.insert("remote_country".to_owned(), val);
        }
        if map.len() > 0 {
            return Some(map);
        }
        None
    }
}

#[cfg(test)]
mod test {
    use super::*;
    use actix_web;
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
        let meta = SenderData::default();
        let log = logging::MozLogger::new_human();
        good_headers.insert(
            http::header::USER_AGENT,
            http::header::HeaderValue::from_static(good_header),
        );
        assert_eq!(
            Some(good_header.to_owned()),
            get_ua(&good_headers, &log, &meta)
        );
        let mut blank_headers = http::HeaderMap::new();
        blank_headers.insert(
            http::header::USER_AGENT,
            http::header::HeaderValue::from_static(blank_header),
        );
        assert_eq!(None, get_ua(&blank_headers, &log, &meta));
        let empty_headers = http::HeaderMap::new();
        assert_eq!(None, get_ua(&empty_headers, &log, &meta));
    }

    #[test]
    fn test_location_good() {
        let test_ip = "63.245.208.195"; // Mozilla
        let log = logging::MozLogger::new_human();
        let langs = vec!["en".to_owned()];
        let mut sender = SenderData::default();
        sender.remote = Some(test_ip.to_owned());
        // TODO: either mock maxminddb::Reader or pass it in as a wrapped impl
        let iploc = maxminddb::Reader::open("mmdb/latest/GeoLite2-City.mmdb").unwrap();
        get_location(&mut sender, &langs, &log, &iploc);
        assert_eq!(sender.city, Some("Sacramento".to_owned()));
        assert_eq!(sender.region, Some("California".to_owned()));
        assert_eq!(sender.country, Some("United States".to_owned()));
    }

    #[test]
    fn test_location_bad() {
        let test_ip = "192.168.1.1";
        let log = logging::MozLogger::new_human();
        let langs = vec!["en".to_owned()];
        let mut sender = SenderData::default();
        sender.remote = Some(test_ip.to_owned());
        // TODO: either mock maxminddb::Reader or pass it in as a wrapped impl
        let iploc = maxminddb::Reader::open("mmdb/latest/GeoLite2-City.mmdb").unwrap();
        get_location(&mut sender, &langs, &log, &iploc);
        assert_eq!(sender.city, None);
        assert_eq!(sender.region, None);
        assert_eq!(sender.country, None);
    }

    #[test]
    fn test_get_remote() {
        let mut headers = actix_web::http::header::HeaderMap::new();
        let mut bad_headers = actix_web::http::header::HeaderMap::new();

        let empty_headers = actix_web::http::header::HeaderMap::new();

        let proxy_list: Vec<IpNet> = vec!["192.168.0.0/24".parse().unwrap()];

        let true_remote: SocketAddr = "1.2.3.4:0".parse().unwrap();
        let proxy_server: SocketAddr = "192.168.0.4:0".parse().unwrap();
        let log = logging::MozLogger::new_human();

        bad_headers.insert(
            http::header::HeaderName::from_lowercase("x-forwarded-for".as_bytes()).unwrap(),
            "".parse().unwrap(),
        );

        // Proxy only, no XFF header
        let remote = get_remote(&Some(proxy_server), &empty_headers, &proxy_list, &log);
        assert!(remote.is_err());

        // Proxy only, bad XFF header
        let remote = get_remote(&Some(proxy_server), &bad_headers, &proxy_list, &log);
        assert!(remote.is_err());

        // Proxy only, crap XFF header
        bad_headers.insert(
            http::header::HeaderName::from_lowercase("x-forwarded-for".as_bytes()).unwrap(),
            "invalid".parse().unwrap(),
        );
        let remote = get_remote(&Some(proxy_server), &bad_headers, &proxy_list, &log);
        assert!(remote.is_err());

        // Peer only, no header
        let remote = get_remote(&Some(true_remote), &empty_headers, &proxy_list, &log);
        assert_eq!(remote.unwrap(), "1.2.3.4".to_owned());

        headers.insert(
            http::header::HeaderName::from_lowercase("x-forwarded-for".as_bytes()).unwrap(),
            "1.2.3.4, 192.168.0.4".parse().unwrap(),
        );

        // Peer proxy, fetch from XFF header
        let remote = get_remote(&Some(proxy_server), &headers, &proxy_list, &log);
        assert_eq!(remote.unwrap(), "1.2.3.4".to_owned());

        // Peer proxy, ensure right most XFF client fetched
        headers.insert(
            http::header::HeaderName::from_lowercase("x-forwarded-for".as_bytes()).unwrap(),
            "1.2.3.4, 2.3.4.5".parse().unwrap(),
        );

        let remote = get_remote(&Some(proxy_server), &headers, &proxy_list, &log);
        assert_eq!(remote.unwrap(), "2.3.4.5".to_owned());

        // Peer proxy, ensure right most non-proxy XFF client fetched
        headers.insert(
            http::header::HeaderName::from_lowercase("x-forwarded-for".as_bytes()).unwrap(),
            "1.2.3.4, 2.3.4.5, 192.168.0.10".parse().unwrap(),
        );

        let remote = get_remote(&Some(proxy_server), &headers, &proxy_list, &log);
        assert_eq!(remote.unwrap(), "2.3.4.5".to_owned());
    }
}
