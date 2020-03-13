SHELL := /bin/bash
PWD := `pwd`

.PHONY: test

test_chan/venv/bin/python:
	pushd test_chan && \
	virtualenv -p python3 venv && \
	venv/bin/pip install --upgrade pip && \
	venv/bin/python setup.py develop && \
	popd

# Fetch the MMDB database and set up the Python integration testing
install: channelserver/mmdb/latest test_chan/venv/bin/python
	echo "Done"

# Do the actual testing.
# TODO: Need to switch this to pytest, however __main__.py does all the same things.
test: channelserver/mmdb/latest test_chan/venv/bin/python
	pushd test_chan && \
	PAIR_MMDB_LOC=../channelserver/mmdb/latest/GeoLite2-City.mmdb venv/bin/python test_chan && \
	popd
