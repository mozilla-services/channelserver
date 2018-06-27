[![Build](https://travis-ci.org/mozilla-services/pairsona.svg?branch=master)](https://travis-ci.org/mozilla-services/pairsona)

# linkserver

Rust based websocket server that provides linking between a sender using HTTP
PUT and a websocket client that is connected.

## Websocket Protocol

Websocket clients must identify with a UUID hex string upon connecting. The
string will be echo'd back upon successful registration.

Clients are then expected to send no other messages (besides Ping frames), and
the server will send relayed payloads.

The default websocket URL is: http://127.0.0.1:8080/ws/

## Sender Protocol

The sender can PUT arbitrary payloads to a connected websocket client that it
knows the hex UUID of.

The sending URL is: http://127.0.0.1:8080/v1/send/{UUID}
