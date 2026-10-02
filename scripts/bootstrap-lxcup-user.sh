#!/usr/bin/env bash
set -Eeuo pipefail

if [[ "${EUID}" -ne 0 ]]; then
  if ! command -v sudo >/dev/null 2>&1; then
    echo "Root-Rechte erforderlich, aber sudo ist nicht installiert. Melde dich als root an und starte dieses Skript erneut mit bash." >&2
    exit 1
  fi
  if [[ ! -f "$0" ]]; then
    echo "Speichere dieses Skript zuerst als Datei und starte es mit bash, damit es per sudo ausgeführt werden kann." >&2
    exit 1
  fi
  echo "Dieses Skript benötigt Root-Rechte und wird per sudo neu gestartet."
  exec sudo -- bash "$0" "$@"
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

# Der Agent und die Bootstrap-Hilfsfunktionen verwenden curl für Downloads und
# Healthchecks. Die Installation ist absichtlich idempotent.
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
  echo "Passwort für ${username} setzen:"
  passwd "${username}"
fi

# Ansible-Playbooks benötigen für become administrative Rechte. Das Passwort
# bleibt auf dem Ziel unverändert und wird vom Worker nur temporär verwendet.
if getent group sudo >/dev/null 2>&1; then
  usermod -aG sudo "${username}"
elif getent group wheel >/dev/null 2>&1; then
  usermod -aG wheel "${username}"
else
  echo "Keine sudo- oder wheel-Gruppe gefunden; administrative Rechte müssen manuell eingerichtet werden." >&2
fi

# Die Docker-Gruppe erlaubt praktisch uneingeschränkte Root-Rechte über den
# Docker-Daemon. Nur auf Docker-Hosts wird der spätere Agent-Dienstbenutzer
# vorbereitet; das Agent-Onboarding stellt die Mitgliedschaft ebenfalls sicher.
if getent group docker >/dev/null 2>&1; then
  echo "Sicherheitshinweis: Docker-Gruppenzugriff ist praktisch root-äquivalent."
  if ! id lxcup-agent >/dev/null 2>&1; then
    useradd --system --user-group --no-create-home --shell /usr/sbin/nologin lxcup-agent
  fi
  usermod -aG docker lxcup-agent
  echo "Docker-Erkennung aktiviert: lxcup-agent ist Mitglied der Gruppe docker."
  if command -v systemctl >/dev/null 2>&1 && systemctl is-active --quiet lxcup-agent.service; then
    systemctl restart lxcup-agent.service
    echo "lxcup-agent wurde neu gestartet, damit die neue Gruppenmitgliedschaft wirksam ist."
  fi
else
  echo "Keine Gruppe docker gefunden; Docker-Zugriff wird beim Agent-Onboarding übersprungen."
fi

echo
echo "Fertig. Ziel im lxcup-Frontend mit folgenden SSH-Daten registrieren:"
echo "  SSH-Benutzer: ${username}"
echo "  Deployment-Secret: das Passwort des SSH-Benutzers"
echo "  Known-Hosts-Secret: die folgenden SSH-Hostschlüssel"
echo "  Verwende bei der Registrierung einen der unten aufgeführten Hostnamen oder eine IP-Adresse."
echo

hostnames="$(hostname)"
read -r -a host_addresses <<< "$(hostname -I 2>/dev/null || true)"
for address in "${host_addresses[@]}"; do
  hostnames+=",${address}"
done

host_keys_found=false
for public_key in /etc/ssh/ssh_host_*_key.pub; do
  [[ -f "${public_key}" ]] || continue
  read -r key_type key_value _ < "${public_key}"
  [[ -n "${key_type}" && -n "${key_value}" ]] || continue
  printf '%s %s %s\n' "${hostnames}" "${key_type}" "${key_value}"
  host_keys_found=true
done
if [[ "${host_keys_found}" == false ]]; then
  echo "Keine öffentlichen SSH-Hostschlüssel gefunden. Prüfe, ob der SSH-Server installiert und eingerichtet ist." >&2
fi
