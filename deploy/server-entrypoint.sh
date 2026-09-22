#!/bin/sh
set -eu

mkdir -p /var/lib/lxcup/secrets
chown -R 10001:10001 /var/lib/lxcup/secrets
exec su -s /bin/sh lxcup -c 'exec /usr/local/bin/lxcup-server'
