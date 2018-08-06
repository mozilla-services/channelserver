# ChannelServer Integration Tester

Hopefully temporary integration test server for the rust ChannelServer

## To use

Since this is python, you need to do the standard python incarnation
incatnations:

```
# Presuming you're in parisona/test_chan:
$ virtualenv .
$ bin/python setup.py install
```

This will create `bin/tester` which will spin up a version of the
channel server from `target/debug` and run a few integration tests on
it.

There are still things to be done here, but at least it works.

