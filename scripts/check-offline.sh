#!/bin/sh
# Fails if a crate that can reach the network appears anywhere in the
# dependency tree, on any platform. quietmouse never talks to the internet;
# this keeps it that way.
set -eu

banned='^(reqwest|hyper|ureq|curl|curl-sys|isahc|surf|attohttpc|h2|tokio|async-std|native-tls|openssl|openssl-sys|rustls|sentry|opentelemetry|tungstenite)$'

found=$(cargo tree --workspace --target all --edges normal,build --prefix none --format '{p}' \
    | awk '{print $1}' | sort -u | grep -E "$banned" || true)

if [ -n "$found" ]; then
    echo "network-capable crates in the dependency tree:" >&2
    echo "$found" >&2
    exit 1
fi
echo "no network-capable crates in the dependency tree"
