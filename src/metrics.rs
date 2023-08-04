//! Metrics tie-ins
//! This is a WIP.

use std::net::UdpSocket;

use cadence::{BufferedUdpMetricSink, NopMetricSink, QueuingMetricSink, StatsdClient};
use slog::{error, info};

use crate::error as c_error;
use crate::logging;
use crate::settings::Settings;

/// Create a cadence StatsdClient from the given options
pub fn metrics_from_opts(
    settings: &Settings,
    log: &logging::MozLogger,
) -> Result<StatsdClient, c_error::HandlerError> {
    let builder = if !settings.statsd_host.is_empty() {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        socket.set_nonblocking(true)?;
        let elements: Vec<&str> = settings.statsd_host.splitn(2, ':').collect();
        let host: (&str, &str) = if elements.len() == 2 {
            (elements[0], elements[1])
        } else {
            (settings.statsd_host.as_str(), "8529")
        };
        let port = host.1.parse::<u16>().unwrap_or(8529);
        let udp_sink = BufferedUdpMetricSink::from((host.0, port), socket)?;
        let sink = QueuingMetricSink::from(udp_sink);
        info!(log.log,
            "Establishing connection to Stat Server";
            "server"=>host.0,
            "port"=>host.1);
        StatsdClient::builder(&settings.metric_name, sink)
    } else {
        info!(log.log, "No Stat Server");
        StatsdClient::builder(&settings.metric_name, NopMetricSink)
    };
    let mlog = log.log.clone();
    Ok(builder
        .with_error_handler(move |err| error!(mlog, "Could not start metrics: {:?}", err))
        .build())
}
