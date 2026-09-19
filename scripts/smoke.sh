#!/usr/bin/env bash
set -euo pipefail

cleanup() {
  docker compose down -v --remove-orphans >/dev/null 2>&1 || true
}
trap cleanup EXIT

DEVELOPMENT_USER_ID="12345678-1234-1234-1234-123456789012"
OTHER_DEVELOPMENT_USER_ID="87654321-4321-4321-4321-210987654321"

memo_curl() {
  curl -H "X-Development-User-Id: $DEVELOPMENT_USER_ID" "$@"
}

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

curl -fsS "http://localhost:8083/api/v1/health/live" \
  | jq -e '.status == "ok"' >/dev/null

ready="$(curl -fsS "http://localhost:8083/api/v1/health/ready")"
jq -e '
  .ready == true
  and .status == "ready"
  and .checks.scylla == "ok"
  and .checks.redis == "ok"
  and .checks.elasticsearch == "ok"
' <<<"$ready" >/dev/null

unauthenticated_status="$(curl -sS -o /tmp/memo-unauthenticated.json -w '%{http_code}' \
  http://localhost:8083/api/v1/memos)"
if [[ "$unauthenticated_status" != "401" ]]; then
  echo "Expected unauthenticated memo request to return 401, got $unauthenticated_status" >&2
  cat /tmp/memo-unauthenticated.json >&2 || true
  exit 1
fi

created="$(memo_curl -fsS \
  -H 'Content-Type: application/json' \
  -d '{"title":"Smoke memo","content":"created by the compose smoke test","tags":["ci","smoke"]}' \
  http://localhost:8083/api/v1/memos)"

memo_id="$(jq -er '.id' <<<"$created")"
version="$(jq -er '.version' <<<"$created")"

memo_curl -fsS "http://localhost:8083/api/v1/memos/$memo_id" | jq -e --arg id "$memo_id" '.id == $id' >/dev/null

frontend_proxy_memo="$(curl -fsS \
  -H "Authorization: Bearer browser-controlled-token" \
  -H "X-Development-User-Id: $OTHER_DEVELOPMENT_USER_ID" \
  "http://localhost:3001/api/v1/memos/$memo_id")"
jq -e --arg id "$memo_id" '.id == $id' <<<"$frontend_proxy_memo" >/dev/null

other_user_status="$(curl -sS \
  -H "X-Development-User-Id: $OTHER_DEVELOPMENT_USER_ID" \
  -o /tmp/memo-other-user.json \
  -w '%{http_code}' \
  "http://localhost:8083/api/v1/memos/$memo_id")"
if [[ "$other_user_status" != "404" ]]; then
  echo "Expected cross-user memo lookup to return 404, got $other_user_status" >&2
  cat /tmp/memo-other-user.json >&2 || true
  exit 1
fi

other_user_list="$(curl -fsS \
  -H "X-Development-User-Id: $OTHER_DEVELOPMENT_USER_ID" \
  http://localhost:8083/api/v1/memos)"
jq -e --arg id "$memo_id" 'all(.id != $id)' <<<"$other_user_list" >/dev/null

other_user_update_status="$(curl -sS \
  -H "X-Development-User-Id: $OTHER_DEVELOPMENT_USER_ID" \
  -H 'Content-Type: application/json' \
  -X PATCH \
  -d "{\"title\":\"cross-user update\",\"content\":\"must not apply\",\"tags\":[\"ci\"],\"version\":$version}" \
  -o /tmp/memo-other-user-update.json \
  -w '%{http_code}' \
  "http://localhost:8083/api/v1/memos/$memo_id")"
if [[ "$other_user_update_status" != "404" ]]; then
  echo "Expected cross-user memo update to return 404, got $other_user_update_status" >&2
  cat /tmp/memo-other-user-update.json >&2 || true
  exit 1
fi

other_user_delete_status="$(curl -sS \
  -H "X-Development-User-Id: $OTHER_DEVELOPMENT_USER_ID" \
  -X DELETE \
  -o /tmp/memo-other-user-delete.json \
  -w '%{http_code}' \
  "http://localhost:8083/api/v1/memos/$memo_id")"
if [[ "$other_user_delete_status" != "404" ]]; then
  echo "Expected cross-user memo delete to return 404, got $other_user_delete_status" >&2
  cat /tmp/memo-other-user-delete.json >&2 || true
  exit 1
fi

memo_curl -fsS "http://localhost:8083/api/v1/memos/$memo_id" \
  | jq -e --arg id "$memo_id" '.id == $id' >/dev/null

docker compose restart backend >/dev/null
wait_for_url "http://localhost:8083/api/v1/health" 60 3

memo_curl -fsS "http://localhost:8083/api/v1/memos/$memo_id" | jq -e --arg id "$memo_id" '.id == $id' >/dev/null

updated="$(memo_curl -fsS \
  -X PATCH \
  -H 'Content-Type: application/json' \
  -d "{\"title\":\"Smoke memo updated\",\"content\":\"updated through the API\",\"tags\":[\"ci\",\"smoke\"],\"version\":$version}" \
  "http://localhost:8083/api/v1/memos/$memo_id")"

jq -e --arg id "$memo_id" '.id == $id and .title == "Smoke memo updated"' <<<"$updated" >/dev/null

search="$(memo_curl -fsS 'http://localhost:8083/api/v1/memos/search?query=updated&tag=smoke&page=1&limit=20')"
jq -e --arg id "$memo_id" '.items | any(.id == $id)' <<<"$search" >/dev/null

bulk_file="$(mktemp)"
for index in $(seq 1 105); do
  pagination_id="$(printf '00000000-0000-4000-8000-%012d' "$index")"
  printf '{"index":{"_index":"memos","_id":"%s"}}\n' "$pagination_id" >>"$bulk_file"
  printf '{"id":"%s","title":"pagination-probe %d","content":"pagination-probe","tags":["pagination"],"user_id":"12345678-1234-1234-1234-123456789012","created_at":"2026-09-18T00:00:00Z","updated_at":"2026-09-18T00:00:00Z","version":1}\n' "$pagination_id" "$index" >>"$bulk_file"
done

curl -fsS \
  -H 'Content-Type: application/x-ndjson' \
  --data-binary @"$bulk_file" \
  'http://localhost:9200/_bulk?refresh=true' \
  | jq -e '.errors == false' >/dev/null
rm -f "$bulk_file"

pagination_search="$(memo_curl -fsS 'http://localhost:8083/api/v1/memos/search?query=pagination-probe&page=6&limit=20')"
jq -e '.total == 105 and .page == 6 and .total_pages == 6 and (.items | length) == 5' <<<"$pagination_search" >/dev/null

expected_version="$(jq -er '.version' <<<"$updated")"
for attempt in $(seq 1 8); do
  (
    memo_curl -sS \
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

memo_curl -fsS -X DELETE "http://localhost:8083/api/v1/memos/$memo_id" >/dev/null

status="$(memo_curl -sS -o /dev/null -w '%{http_code}' "http://localhost:8083/api/v1/memos/$memo_id")"
if [[ "$status" != "404" ]]; then
  echo "Expected deleted memo lookup to return 404, got $status" >&2
  exit 1
fi

resilience_created="$(memo_curl -fsS \
  -H 'Content-Type: application/json' \
  -d '{"title":"Resilience memo","content":"survives secondary store outages","tags":["ci","resilience"]}' \
  http://localhost:8083/api/v1/memos)"
resilience_id="$(jq -er '.id' <<<"$resilience_created")"

docker compose stop redis elasticsearch >/dev/null

degraded_ready="$(curl -fsS "http://localhost:8083/api/v1/health/ready")"
jq -e '
  .ready == true
  and .status == "degraded"
  and .checks.scylla == "ok"
  and .checks.redis == "down"
  and .checks.elasticsearch == "down"
' <<<"$degraded_ready" >/dev/null

memo_curl -fsS "http://localhost:8083/api/v1/memos/$resilience_id" \
  | jq -e --arg id "$resilience_id" '.id == $id' >/dev/null

outage_created="$(memo_curl -fsS \
  -H 'Content-Type: application/json' \
  -d '{"title":"Outage write","content":"Scylla remains authoritative","tags":["ci","resilience"]}' \
  http://localhost:8083/api/v1/memos)"
outage_id="$(jq -er '.id' <<<"$outage_created")"

memo_curl -fsS -X DELETE "http://localhost:8083/api/v1/memos/$resilience_id" >/dev/null
memo_curl -fsS "http://localhost:8083/api/v1/memos/$outage_id" \
  | jq -e --arg id "$outage_id" '.id == $id' >/dev/null

docker compose restart backend >/dev/null
wait_for_url "http://localhost:8083/api/v1/health/live" 60 3
restart_degraded="$(curl -fsS "http://localhost:8083/api/v1/health/ready")"
jq -e '
  .ready == true
  and .status == "degraded"
  and .checks.scylla == "ok"
  and .checks.redis == "down"
  and .checks.elasticsearch == "down"
' <<<"$restart_degraded" >/dev/null

docker compose start redis elasticsearch >/dev/null

recovery_ready=0
for _ in $(seq 1 60); do
  if curl -fsS "http://localhost:8083/api/v1/health/ready" \
    | jq -e '.status == "ready"' >/dev/null 2>&1; then
    recovery_ready=1
    break
  fi
  sleep 2
done
if [[ "$recovery_ready" -ne 1 ]]; then
  echo "Secondary stores did not recover to ready state" >&2
  exit 1
fi

projection_recovered=0
for _ in $(seq 1 60); do
  if outage_search="$(memo_curl -fsS 'http://localhost:8083/api/v1/memos/search?query=Outage%20write&page=1&limit=20' 2>/dev/null)" \
    && deleted_search="$(memo_curl -fsS 'http://localhost:8083/api/v1/memos/search?query=Resilience%20memo&page=1&limit=20' 2>/dev/null)"; then
    if jq -e --arg id "$outage_id" '.items | any(.id == $id)' <<<"$outage_search" >/dev/null \
      && jq -e --arg id "$resilience_id" '.items | all(.id != $id)' <<<"$deleted_search" >/dev/null; then
      projection_recovered=1
      break
    fi
  fi
  sleep 2
done
if [[ "$projection_recovered" -ne 1 ]]; then
  echo "Projection reconciliation did not recover create/delete changes after secondary stores returned" >&2
  docker compose logs --no-color --tail=200 backend elasticsearch redis >&2 || true
  exit 1
fi

docker compose stop scylla >/dev/null
unavailable_body="$(mktemp)"
ready_status="$(curl -sS -o "$unavailable_body" -w '%{http_code}' "http://localhost:8083/api/v1/health/ready")"
if [[ "$ready_status" != "503" ]]; then
  echo "Expected readiness to return 503 when Scylla is down, got $ready_status" >&2
  cat "$unavailable_body" >&2 || true
  rm -f "$unavailable_body"
  exit 1
fi
jq -e '
  .ready == false
  and .status == "unavailable"
  and .checks.scylla == "down"
' "$unavailable_body" >/dev/null
rm -f "$unavailable_body"

echo "Compose smoke test passed"
