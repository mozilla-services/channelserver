#!/bin/bash
# Fetch the latest MaxMind GeoLite2 City Database
# This will store the db in the ./mmdb/latest aliased directory
#
mkdir -p mmdb
pushd mmdb
wget http://geolite.maxmind.com/download/geoip/database/GeoLite2-City.tar.gz
tar -zxvf GeoLite2-City.tar.gz
if [ -e latest ]; then rm latest; fi
latest=`ls -1ad --color=never GeoLite2-City_*|tail -1`
ln -s $latest latest
popd
