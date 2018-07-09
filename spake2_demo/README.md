[![Build](https://travis-ci.org/mozilla-services/pairsona.svg?branch=master)](https://travis-ci.org/mozilla-services/pairsona)

# SPAKE2 Pairing Demo

This is a quick-n-dirty demo of using
the SPAKE2 protocol to bootstrap an encrypted channel
on top of the message pipes provided by [linkserver](../linkserver/).

It uses [spake2.rs](https://github.com/warner/spake2.rs) for the crypto bits,
compiled for the web via `cargo web`.  To run it, you will need to:

0. Install the cargo web extension, if not already installed.
```
#> cargo install -f cargo-web
```

1. Build and run linkserver:

```
$> cd ../linkserver/
$> cargo run
```

2. Build and run this demo code via cargo-web:

```
$> cd ../spake2_demo/
$> cargo web start
```

3. Visit http://localhost:8000 in your web browser.

If all went well, you should be invited to pair
two webpages and make a little chat program.

