[![License: MPL 2.0](https://img.shields.io/badge/License-MPL%202.0-brightgreen.svg)](https://opensource.org/licenses/MPL-2.0)
[![Build](https://travis-ci.org/mozilla-services/channelserver.svg?branch=master)](https://travis-ci.org/mozilla-services/channelserver)
[![Connect to Matrix via the Riot webapp][matrix-badge]][matrix]

# ChannelServer

A project to make Firefox Account log-in, pairing and sync easier
between devices or applications.

Contains:

- [channelserver](./channelserver/) - websocket message relay server.
- [test_chan](./test_chan/) - python based external integration tester
  for channelserver.

For client code that uses this facility to create
an encrypted and authenticated channel between two
client devices, see [fxa-pairing-channel](https://github.com/mozilla/fxa-pairing-channel).

[matrix-badge]: https://img.shields.io/badge/chat%20on%20[m]-%23services%3Amozilla.org-blue
[matrix]: https://chat.mozilla.org/#/room/#services:mozilla.org
