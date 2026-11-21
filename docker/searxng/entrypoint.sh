#!/bin/sh
# SearxNG entrypoint wrapper.
#
# The tracked settings are mounted read-only; nothing about a running instance — above
# all the secret_key that signs its sessions — is kept in git. On first start this writes
# a random secret_key into the config volume, and on every start it renders the tracked
# settings into that volume with the secret under `server:`. The secret therefore
# survives restart, `up` and `down`, and an edit to the tracked settings takes effect on
# the next start. The stock entrypoint is exec'd afterwards.
set -eu

config_dir=/etc/searxng
tracked_dir=/opt/libra-searxng
rendered=$config_dir/settings.yml

if [ ! -s "$config_dir/secret_key" ]; then
    head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$config_dir/secret_key"
    chmod 600 "$config_dir/secret_key"
fi
secret=$(cat "$config_dir/secret_key")

# A Windows checkout gives this file CRLF line endings, and `^server:$` would then miss.
# Strip CR before matching, and stop rather than exec the stock entrypoint, which leaves an
# existing settings.yml alone: a miss here means SearxNG runs on its public default key.
tr -d '\r' < "$tracked_dir/settings.yml" \
    | sed "s|^server:\$|server:\n  secret_key: \"${secret}\"|" > "$rendered"

if ! grep -q '^  secret_key:' "$rendered"; then
    echo "entrypoint: no server: block to put the secret_key in ($tracked_dir/settings.yml)" >&2
    exit 1
fi

exec /usr/local/searxng/entrypoint.sh "$@"
