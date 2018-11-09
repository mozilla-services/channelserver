SHELL := /bin/bash
PWD := `pwd`

.PHONY: test

channelserver/mmdb/latest:
	pushd channelserver && \
	. fetch_mmdb.bash && \
	popd

test_chan/bin/pytest:
	pushd test_chan && \
	virtualenv -p python3 . && \
	bin/pip install --upgrade pip && \
	bin/python setup.py develop && \
	popd

install: channelserver/mmdb/latest test_chan/bin/pytest
	echo "Done"

test: channelserver/mmdb/latest test_chan/bin/pytest
	ls -l target/debug/
	pushd test_chan && \
	echo "### `pwd`" && \
	ls -l ../mmdb && \
	bin/pytest -sv test_chan && \
	popd
