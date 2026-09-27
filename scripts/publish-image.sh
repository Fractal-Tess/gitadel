#!/usr/bin/env bash
set -euo pipefail

# Usage: ./scripts/publish-image.sh [TAG ...] (defaults to latest).
# Requires Docker and Skopeo. Existing Docker logins are reused.
# Optional: gh supplies GHCR credentials; GITADEL_TOKEN_FILE supplies Gitadel's.
ROOT=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
GHCR_IMAGE=${GHCR_IMAGE:-ghcr.io/fractal-tess/gitadel}
GITADEL_IMAGE=${GITADEL_IMAGE:-neo.netbird.cloud:3030/fractal-tess/gitadel}
DOCKERHUB_IMAGE=${DOCKERHUB_IMAGE:-docker.io/vgfractal/gitadel}
GITADEL_TLS_VERIFY=${GITADEL_TLS_VERIFY:-true}
LOCAL_IMAGE=${LOCAL_IMAGE:-gitadel:publish}

if [[ ${1:-} == --help ]]; then
    printf '%s\n' 'Usage: scripts/publish-image.sh [TAG ...]' \
        'Build once and publish each tag to GHCR, Gitadel, and Docker Hub.' \
        'Defaults: ghcr.io/fractal-tess/gitadel, neo.netbird.cloud:3030/fractal-tess/gitadel, docker.io/vgfractal/gitadel.' \
        'Overrides: GHCR_IMAGE, GITADEL_IMAGE, DOCKERHUB_IMAGE, LOCAL_IMAGE.' \
        'Authentication: existing Docker logins, gh (optional), GITADEL_TOKEN_FILE and GITADEL_USERNAME.' \
        'For HTTP Gitadel over the private mesh, set GITADEL_TLS_VERIFY=false explicitly.'
    exit 0
fi
(($#)) || set -- latest
for tag in "$@"; do
    if [[ ! $tag =~ ^[a-zA-Z0-9_][a-zA-Z0-9_.-]{0,127}$ ]]; then
        printf 'Invalid image tag: %s\n' "$tag" >&2
        exit 1
    fi
done
for command in docker skopeo; do
    command -v "$command" >/dev/null || { printf 'Missing command: %s\n' "$command" >&2; exit 1; }
done

# Keep any automatically acquired tokens out of persistent Docker configuration.
umask 077
auth_dir=$(mktemp -d)
trap 'rm -rf -- "$auth_dir"' EXIT
auth_file=$auth_dir/auth.json
# The source is our own unsigned local build, never a downloaded image.
printf '%s\n' '{"default":[{"type":"reject"}],"transports":{"docker-daemon":{"":[{"type":"insecureAcceptAnything"}]}}}' > "$auth_dir/policy.json"
docker_config=${DOCKER_CONFIG:-$HOME/.docker}/config.json
if [[ -f $docker_config ]]; then
    cp -- "$docker_config" "$auth_file"
else
    printf '{"auths":{}}\n' > "$auth_file"
fi
if [[ $GHCR_IMAGE == ghcr.io/* ]] && command -v gh >/dev/null; then
    gh auth token | skopeo login --authfile "$auth_file" --username "$(gh api user --jq .login)" --password-stdin ghcr.io
fi
if [[ -n ${GITADEL_TOKEN_FILE:-} ]]; then
    skopeo login --authfile "$auth_file" --tls-verify="$GITADEL_TLS_VERIFY" \
        --username "${GITADEL_USERNAME:?Set GITADEL_USERNAME with GITADEL_TOKEN_FILE}" \
        --password-stdin "${GITADEL_IMAGE%%/*}" < "$GITADEL_TOKEN_FILE"
fi

docker build --tag "$LOCAL_IMAGE" "$ROOT"
failed=0
for image in "$GHCR_IMAGE" "$GITADEL_IMAGE" "$DOCKERHUB_IMAGE"; do
    tls_verify=true
    [[ $image != "$GITADEL_IMAGE" ]] || tls_verify=$GITADEL_TLS_VERIFY
    for tag in "$@"; do
        printf '\nPublishing %s:%s\n' "$image" "$tag"
        if skopeo --policy "$auth_dir/policy.json" copy --dest-authfile "$auth_file" --dest-tls-verify="$tls_verify" \
            "docker-daemon:$LOCAL_IMAGE" "docker://$image:$tag"; then
            if ! skopeo inspect --no-tags --authfile "$auth_file" --tls-verify="$tls_verify" \
                --format '{{.Digest}}' "docker://$image:$tag"; then
                failed=1
            fi
        else
            printf 'Failed to publish %s:%s; continuing with remaining destinations.\n' "$image" "$tag" >&2
            failed=1
        fi
    done
done
exit "$failed"
