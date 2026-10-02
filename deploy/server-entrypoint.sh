#!/bin/sh
set -eu

mkdir -p /var/lib/lxcup/secrets
mkdir -p /var/lib/lxcup/backups
# Remove only private scratch paths reserved for interrupted backup creation.
find /var/lib/lxcup/backups -mindepth 1 -maxdepth 1 -type d -name '.staging-*' -exec rm -rf -- {} +
find /var/lib/lxcup/backups -mindepth 1 -maxdepth 1 -type f -name '.lxcup-*.partial' -delete
chown -R 10001:10001 /var/lib/lxcup/secrets
chown 10001:10001 /var/lib/lxcup/backups
exec su -s /bin/sh lxcup -c 'exec /usr/local/bin/lxcup-server'
