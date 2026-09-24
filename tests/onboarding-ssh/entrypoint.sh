#!/bin/sh
set -eu

: "${LXCUP_E2E_SSH_PASSWORD:?LXCUP_E2E_SSH_PASSWORD must be set}"
printf 'lxcup:%s\n' "$LXCUP_E2E_SSH_PASSWORD" | chpasswd
ssh-keygen -A
exec /sbin/init
