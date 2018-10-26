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

You can test using ``bin/pytest test_chan``. If you need to configure
things, you need to set environment variables:

::

    *TEST_APP_PATH* - Path to channel-server application. Set to "" to

skip starting the local channel server. *../target/debug/channelserver*

::

    *
