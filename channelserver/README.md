# Simple websocket channel server

## Config options:

Options can be set as ENV vars (prefixed with "`PAIR_`", e.g.
"`PAIR_PORT="8000"`"), or as arguments ("`--port=8000`").

See `src/settings.rs` for defaults.

## GeoIP lookup
This product includes GeoLite2 data created by MaxMind, available from
[https://www.maxmind.com](https://www.maxmind.com).

This will require a [maxmind GeoIP](https://dev.maxmind.com/geoip/geoip2/geolite2/) lookup database. This presumes that
the database will be in `mmdb/latest/GeoLite2-City.mmdb`. Use the
`mmdb_loc` to specify a different path (*Note:* if running in the debugger,
you may need to create a symlink under `target/debug`.)

## Compile and run:

After installing rust via [rustup](https://rustup.rs/):

```sh
$ cargo run
```

## API

When connecting to the server as a new session, the first response message contains a JSON response containing the URI path to send to the counterpart client, this is known as the "channel", and the discrete channelID.

e.g. for a connection to `wss://example.com/v1/ws/`
```json
{"channelid":"IZ5B8Wj2qR1NlsNbSXQ2Fg","link":"/v1/ws/IZ5B8Wj2qR1NlsNbSXQ2Fg"}
```
Additional connections can be made to the URI specified in `link`.

Messages sent are expected to be URL Safe base64 encoded blocks and are delivered wrapped in a JSON envelope containing the message and sender meta data.

e.g.
```json
{"message":"aBc12e....","sender":{"city":"Mountain View","country":"USA","region":"California","remote":"10.0.0.1", }}
```

This will attempt to localize the geolocation data based on the preferred `Accept-Languages:` HTTP header. If no header is provided, results are unspecified (although probably in German). If an aspect of the location cannot be determined, it is not included in the output.

There are several limitations put in place and controlled by the following options:

`max_exchanges` (env: **PAIR_MAX_EXCHANGES**) - Limit the max number of messages that can be exchanged across a channel. (default: 10)

`conn_lifespan` (env: **PAIR_CONN_LIFESPAN**) - Limit the max lifespan of a give channel to this many seconds. The clock starts when the channel is first created. (default: 300)

`client_timeout` (env: **PAIR_CLIENT_TIMEOUT**) - How often to check to see if a client connection has been closed. This can happen due to any number of reasons, but mostly because the internet hates long lived things. (default: 30)

`max_channel_connections` (env: **PAIR_MAX_CHANNEL_CONNECTIONS**) - Max number of connections to a given channel. *NOTE* after the second connection, subsequent connections must be from one of the previously connected IP addresses. (default: 3)


Additional settings are described in `src/settings.rs`

This version of the server will echo data sent to a channel all other
sessions on a channel. This will change in later versions.

## Stats Collected:

* **conn.create** - New connection created
* **conn.expired** - Connection terminated, channel lifespan expired
* **conn.max.data** - Connection terminated due to too much data in channel
* **conn.max.msg** - Connection terminated due to many messages exchanged through channel
* **conn.timeout** - Connection terminated because of heartbeat timeout
