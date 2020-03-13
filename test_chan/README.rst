ChannelServer Integration Tester
================================

Hopefully temporary integration test server for the rust ChannelServer

To use
------

Since this is python, you need to do the standard python incarnation
incatnations:

::

    # Presuming you're in parisona/test_chan:
    $ virtualenv .
    $ bin/python setup.py install

You will need to specify the mmdb database location for the channelserver. By default, channelserver
will try to look in ``./mmdb/latest/GeoLite2_City.mmdb``.

You can test using ``bin/pytest test_chan``. If you need to configure
things, you need to set environment variables:

+------------------------+-------------------------------------+---------------------------------+
| Variable               | Description                         | Default                         |
+========================+=====================================+=================================+
|*TEST_APP_PATH*         | Path to channel-server application. | _../target/debug/channelserver_ |
|                        | Set to "" to skip starting the local|                                 |
|                        | channel server.                     |                                 |
+------------------------+-------------------------------------+---------------------------------+
|*TEST_PROTOCOL*         | Websocket protocol to use (ws, wss) | _ws_                            |
+------------------------+-------------------------------------+---------------------------------+
|*TEST_HOST*             | Host name to connect to             | _localhost_                     |
+------------------------+-------------------------------------+---------------------------------+
|*TEST_PORT*             | Port number for connection          | _8000_                          |
+------------------------+-------------------------------------+---------------------------------+
|*TEST_MAX_TRANSACTIONS* | Number of transactions to attempt   | 5                               |
|                        | for the `test_max_transactions`     |                                 |
+------------------------+-------------------------------------+---------------------------------+
|*TEST_MAX_DATA*         | Max Data to attempt to send (0 to   | 0                               |
|                        | skip this test)                     |                                 |
+------------------------+-------------------------------------+---------------------------------+

