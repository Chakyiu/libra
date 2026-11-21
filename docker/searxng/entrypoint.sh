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

if [ ! -s "$config_dir/secret_key" ]; then
    head -c 32 /dev/urandom | od -An -tx1 | tr -d ' \n' > "$config_dir/secret_key"
    chmod 600 "$config_dir/secret_key"
fi

secret=$(cat "$config_dir/secret_key")
sed "s|^server:\$|server:\n  secret_key: \"${secret}\"|" \
    "$tracked_dir/settings.yml" > "$config_dir/settings.yml"

exec /usr/local/searxng/entrypoint.sh "$@"
