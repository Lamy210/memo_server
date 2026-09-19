#!/usr/bin/env bash
set -euo pipefail

baseline_dir="${1:?baseline directory is required}"
candidate_dir="${2:?candidate directory is required}"
output_dir="${3:?output directory is required}"

mkdir -p "$output_dir"

summary="$output_dir/summary.md"
{
  echo "## UI visual diff"
  echo
  echo "| Screen | Changed pixels |"
  echo "| --- | ---: |"
} > "$summary"

screens=(memos search new edit unauthorized)

for screen in "${screens[@]}"; do
  before="$baseline_dir/$screen.png"
  after="$candidate_dir/$screen.png"
  diff="$output_dir/$screen-diff.png"
  side_by_side="$output_dir/$screen-before-after.png"
  metric_file="$output_dir/$screen.metric"

  if [[ ! -f "$before" || ! -f "$after" ]]; then
    echo "Missing screenshot for $screen" >&2
    exit 1
  fi

  set +e
  compare -metric AE "$before" "$after" "$diff" 2> "$metric_file"
  compare_status=$?
  set -e

  # ImageMagick returns 1 when pixels differ and >1 for an execution error.
  if [[ "$compare_status" -gt 1 ]]; then
    cat "$metric_file" >&2
    exit "$compare_status"
  fi

  changed_pixels="$(tr -d '[:space:]' < "$metric_file")"
  [[ -n "$changed_pixels" ]] || changed_pixels="0"

  convert "$before" "$after" +append "$side_by_side"
  printf '| `%s` | %s |\n' "$screen" "$changed_pixels" >> "$summary"
done

{
  echo
  echo "Artifacts include the base screenshot, PR screenshot, pixel diff, and side-by-side image for every screen."
  echo "Pixel differences are informational and do not fail the PR."
} >> "$summary"

cat "$summary"
