#!/usr/bin/env bash
set -euo pipefail

cleanup() {
  docker compose down -v --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

wait_for_url() {
  local url="$1"
  local attempts="${2:-90}"
  local sleep_seconds="${3:-5}"

  for ((attempt = 1; attempt <= attempts; attempt++)); do
    if curl -fsS "$url" >/dev/null; then
      return 0
    fi
    sleep "$sleep_seconds"
  done

  echo "Timed out waiting for $url" >&2
  docker compose ps >&2 || true
  docker compose logs --no-color --tail=200 >&2 || true
  return 1
}

if ! docker compose up -d --build; then
  echo "Docker Compose startup failed" >&2
  docker compose ps >&2 || true
  docker compose logs --no-color --tail=300 scylla backend elasticsearch redis frontend >&2 || true
  exit 1
fi

wait_for_url "http://localhost:8083/api/v1/health" 120 5
wait_for_url "http://localhost:3001/memos" 60 3
wait_for_url "http://localhost:3001/api/v1/health" 30 2

created="$(curl -fsS \
  -H 'Content-Type: application/json' \
  -d '{"title":"Smoke memo","content":"created by the compose smoke test","tags":["ci","smoke"]}' \
  http://localhost:8083/api/v1/memos)"

memo_id="$(jq -er '.id' <<<"$created")"
version="$(jq -er '.version' <<<"$created")"

curl -fsS "http://localhost:8083/api/v1/memos/$memo_id" | jq -e --arg id "$memo_id" '.id == $id' >/dev/null

docker compose restart backend >/dev/null
wait_for_url "http://localhost:8083/api/v1/health" 60 3

curl -fsS "http://localhost:8083/api/v1/memos/$memo_id" | jq -e --arg id "$memo_id" '.id == $id' >/dev/null

updated="$(curl -fsS \
  -X PATCH \
  -H 'Content-Type: application/json' \
  -d "{\"title\":\"Smoke memo updated\",\"content\":\"updated through the API\",\"tags\":[\"ci\",\"smoke\"],\"version\":$version}" \
  "http://localhost:8083/api/v1/memos/$memo_id")"

jq -e --arg id "$memo_id" '.id == $id and .title == "Smoke memo updated"' <<<"$updated" >/dev/null

search="$(curl -fsS 'http://localhost:8083/api/v1/memos/search?query=updated&tag=smoke&page=1&limit=20')"
jq -e --arg id "$memo_id" '.items | any(.id == $id)' <<<"$search" >/dev/null

expected_version="$(jq -er '.version' <<<"$updated")"
for attempt in $(seq 1 8); do
  (
    curl -sS \
      -o "/tmp/memo-conflict-body-$attempt.json" \
      -w '%{http_code}' \
      -X PATCH \
      -H 'Content-Type: application/json' \
      -d "{\"title\":\"Concurrent update $attempt\",\"content\":\"concurrency probe\",\"tags\":[\"ci\",\"smoke\"],\"version\":$expected_version}" \
      "http://localhost:8083/api/v1/memos/$memo_id" \
      > "/tmp/memo-conflict-status-$attempt"
  ) &
done
wait

success_count=0
conflict_count=0
for attempt in $(seq 1 8); do
  code="$(cat "/tmp/memo-conflict-status-$attempt")"
  case "$code" in
    200) success_count=$((success_count + 1)) ;;
    409) conflict_count=$((conflict_count + 1)) ;;
    *)
      echo "Unexpected status from concurrent update $attempt: $code" >&2
      cat "/tmp/memo-conflict-body-$attempt.json" >&2 || true
      exit 1
      ;;
  esac
done

if [[ "$success_count" -ne 1 || "$conflict_count" -ne 7 ]]; then
  echo "Expected one successful concurrent update and seven conflicts; got $success_count success(es), $conflict_count conflict(s)" >&2
  exit 1
fi

curl -fsS -X DELETE "http://localhost:8083/api/v1/memos/$memo_id" >/dev/null

status="$(curl -sS -o /dev/null -w '%{http_code}' "http://localhost:8083/api/v1/memos/$memo_id")"
if [[ "$status" != "404" ]]; then
  echo "Expected deleted memo lookup to return 404, got $status" >&2
  exit 1
fi

echo "Compose smoke test passed"
