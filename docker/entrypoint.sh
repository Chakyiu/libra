#!/bin/sh
set -eu

if ! chown -R 10001:10001 /data || ! chmod -R u+rwX /data; then
    echo "libra: cannot prepare /data for uid 10001 (chown or chmod failed); ensure the mount is writable and permits ownership/permission changes, or pre-own it as 10001:10001" >&2
    exit 1
fi

exec setpriv --reuid=10001 --regid=10001 --keep-groups "$@"
