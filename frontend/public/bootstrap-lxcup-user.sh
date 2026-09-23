#!/usr/bin/env bash
# Served copy of scripts/bootstrap-lxcup-user.sh for the target registration UI.
# Keep both files functionally identical.
set -Eeuo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  echo "Dieses Skript muss als root ausgeführt werden." >&2
  exit 1
fi

install_package() {
  local package="$1"
  if command -v apt-get >/dev/null 2>&1; then
    apt-get update
    DEBIAN_FRONTEND=noninteractive apt-get install --yes "$package"
  elif command -v dnf >/dev/null 2>&1; then
    dnf install --assumeyes "$package"
  elif command -v yum >/dev/null 2>&1; then
    yum install --assumeyes "$package"
  else
    echo "Das Paket ${package} wurde nicht gefunden und der Paketmanager ist unbekannt." >&2
    exit 1
  fi
}

if ! command -v sudo >/dev/null 2>&1; then
  install_package sudo
fi
if ! command -v curl >/dev/null 2>&1; then
  install_package curl
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
