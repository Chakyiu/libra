#!/bin/sh
set -eu

uid="$(id -u)"
if [ "$uid" = 10001 ]; then
    if [ -w /data ]; then
        exec "$@"
    fi
    echo "libra: /data is not writable by uid 10001; make the mount writable and owned by uid/gid 10001" >&2
    exit 1
fi

if [ "$uid" != 0 ]; then
    echo "libra: cannot prepare /data as uid $uid; start as root or uid 10001 with a writable mount" >&2
    exit 1
fi

if ! chown -R 10001:10001 /data || ! chmod -R u+rwX /data; then
    echo "libra: cannot prepare /data for uid 10001 (chown or chmod failed); ensure the mount is writable and permits ownership/permission changes, or pre-own it as 10001:10001" >&2
    exit 1
fi

exec setpriv --reuid=10001 --regid=10001 --keep-groups "$@"
