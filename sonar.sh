#!/usr/bin/env bash

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

if [[ $# -lt 1 || $# -gt 3 ]]; then
    cat >&2 <<'EOF'
Verwendung:
./sonar.sh http://sonarqube:9000 [token] [project-key]
EOF
    exit 1
fi

SONAR_HOST_URL="${1%/}"
SONAR_TOKEN_VALUE="${2:-${SONAR_TOKEN:-}}"
SONAR_PROJECT_KEY="${3:-3D-Online-Schach}"

if [[ ! "${SONAR_HOST_URL}" =~ ^https?://[^[:space:]]+$ ]]; then
    echo "Die SonarQube-URL muss eine vollständige HTTP- oder HTTPS-URL sein." >&2
    exit 1
fi

NODE="$(command -v node || true)"
if [[ -z "${NODE}" ]]; then
    echo "node wurde nicht gefunden. Installiere zuerst Node.js." >&2
    exit 1
fi

SONAR_PROJECT_VERSION="$("${NODE}" "${SCRIPT_DIR}/scripts/read-version.mjs")"
if [[ -z "${SONAR_PROJECT_VERSION}" ]]; then
    echo "Die Anwendungsversion konnte nicht gelesen werden." >&2
    exit 1
fi

echo "Erzeuge LCOV-Coverage-Berichte für alle Workspace-Pakete ..."
"${NODE}" "${SCRIPT_DIR}/scripts/run-coverage.mjs"

SONAR_SCANNER="$(command -v sonar-scanner || true)"
DOCKER="$(command -v docker || true)"

echo "Starte SonarQube-Analyse für '${SONAR_PROJECT_KEY}' ..."
echo "Server: ${SONAR_HOST_URL}"
echo "Version: ${SONAR_PROJECT_VERSION}"

if [[ -n "${SONAR_SCANNER}" ]]; then
    if [[ -n "${SONAR_TOKEN_VALUE}" ]]; then
        SONAR_TOKEN="${SONAR_TOKEN_VALUE}" "${SONAR_SCANNER}" \
            "-Dsonar.host.url=${SONAR_HOST_URL}" \
            "-Dsonar.projectKey=${SONAR_PROJECT_KEY}" \
            "-Dsonar.projectVersion=${SONAR_PROJECT_VERSION}"
    else
        "${SONAR_SCANNER}" \
            "-Dsonar.host.url=${SONAR_HOST_URL}" \
            "-Dsonar.projectKey=${SONAR_PROJECT_KEY}" \
            "-Dsonar.projectVersion=${SONAR_PROJECT_VERSION}"
    fi
elif [[ -n "${DOCKER}" ]]; then
    DOCKER_ARGS=(
        run
        --rm
        -v "${SCRIPT_DIR}:/usr/src"
        -w /usr/src
    )
    SONAR_HOST_NAME="${SONAR_HOST_URL#*://}"
    SONAR_HOST_NAME="${SONAR_HOST_NAME%%:*}"
    SONAR_HOST_IP="$(getent ahostsv4 "${SONAR_HOST_NAME}" 2>/dev/null | awk 'NR == 1 { print $1 }')"
    if [[ -n "${SONAR_HOST_IP}" && "${SONAR_HOST_IP}" != 127.* ]]; then
        DOCKER_ARGS+=( --add-host "${SONAR_HOST_NAME}:${SONAR_HOST_IP}" )
    fi
    if [[ -n "${SONAR_TOKEN_VALUE}" ]]; then
        DOCKER_ARGS+=( -e "SONAR_TOKEN=${SONAR_TOKEN_VALUE}" )
    fi
    "${DOCKER}" "${DOCKER_ARGS[@]}" sonarsource/sonar-scanner-cli:latest \
        "-Dsonar.host.url=${SONAR_HOST_URL}" \
        "-Dsonar.projectKey=${SONAR_PROJECT_KEY}" \
        "-Dsonar.projectVersion=${SONAR_PROJECT_VERSION}"
else
    echo "Weder sonar-scanner noch Docker wurde gefunden." >&2
    exit 1
fi

echo "SonarQube-Analyse erfolgreich abgeschlossen."
