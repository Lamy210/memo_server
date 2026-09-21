#!/usr/bin/env bash
set -euo pipefail

if [[ "$#" -lt 3 ]]; then
  echo "usage: $0 <attempts> <base-delay-seconds> <command> [args...]" >&2
  exit 2
fi

attempts="$1"
base_delay_seconds="$2"
shift 2

if ! [[ "$attempts" =~ ^[1-9][0-9]*$ ]]; then
  echo "attempts must be a positive integer, got: $attempts" >&2
  exit 2
fi

if ! [[ "$base_delay_seconds" =~ ^[0-9]+$ ]]; then
  echo "base delay must be a non-negative integer, got: $base_delay_seconds" >&2
  exit 2
fi

for ((attempt = 1; attempt <= attempts; attempt++)); do
  if "$@"; then
    exit 0
  fi

  if [[ "$attempt" -eq "$attempts" ]]; then
    echo "command failed after $attempt attempts" >&2
    exit 1
  fi

  retry_delay=$((attempt * base_delay_seconds))
  echo "command failed on attempt $attempt/$attempts; retrying in ${retry_delay}s" >&2
  sleep "$retry_delay"
done
