#!/usr/bin/env bash
set -Eeuo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "Dieses Skript muss als root ausgeführt werden." >&2
  exit 1
fi

if ! command -v sudo >/dev/null 2>&1; then
  if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install --yes sudo
  elif command -v dnf >/dev/null 2>&1; then
    dnf install --assumeyes sudo
  elif command -v yum >/dev/null 2>&1; then
    yum install --assumeyes sudo
  else
    echo "sudo wurde nicht gefunden und der Paketmanager ist unbekannt." >&2
    exit 1
  fi
fi

username="${1:-lxcup}"
if [[ ! "${username}" =~ ^[a-z_][a-z0-9_-]{0,31}$ ]]; then
  echo "Ungültiger Benutzername: ${username}" >&2
  exit 1
fi

if id "${username}" >/dev/null 2>&1; then
  echo "Benutzer ${username} existiert bereits."
else
  useradd --create-home --shell /bin/bash "${username}"
  echo "Benutzer ${username} wurde angelegt."
fi

echo "Passwort für ${username} setzen:"
passwd "${username}"

# Ansible-Playbooks benötigen für become administrative Rechte. Das Passwort
# bleibt auf dem Ziel unverändert und wird vom Worker nur temporär verwendet.
if getent group sudo >/dev/null 2>&1; then
  usermod -aG sudo "${username}"
elif getent group wheel >/dev/null 2>&1; then
  usermod -aG wheel "${username}"
else
  echo "Keine sudo- oder wheel-Gruppe gefunden; administrative Rechte müssen manuell eingerichtet werden." >&2
fi

echo
echo "Fertig. Ziel im lxcup-Frontend mit folgenden SSH-Daten registrieren:"
echo "  SSH-Benutzer: ${username}"
echo "  Deployment-Secret: das soeben gesetzte Passwort"
echo "  Known-Hosts-Secret: Ausgabe von scripts/get-ssh-known-hosts.ps1"
