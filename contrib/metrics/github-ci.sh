#!/bin/sh
# Shows how the latest GitHub Actions runs of a branch went on a dashboard panel.
#
#   github-ci.sh owner/repo [branch] [metric-id]
#   ssp dashboard --widgets clock,metric:ci,cpu
#
# Needs the GitHub CLI (`gh`, logged in) and `ssp`. Run it every few minutes, e.g. from cron:
#
#   */5 * * * * /path/to/github-ci.sh owner/repo main ci
#
# The panel shows the number of failed runs among the last 20, with a graph of it over time, and
# how many runs are in progress.
set -eu

repo=${1:?usage: github-ci.sh owner/repo [branch] [metric-id]}
branch=${2:-main}
id=${3:-ci}

counts=$(gh run list --repo "$repo" --branch "$branch" --limit 20 \
  --json status,conclusion \
  --jq '[length,
         ([.[] | select(.conclusion == "failure")] | length),
         ([.[] | select(.status != "completed")] | length)] | @tsv')
total=$(printf '%s' "$counts" | cut -f1)
failed=$(printf '%s' "$counts" | cut -f2)
running=$(printf '%s' "$counts" | cut -f3)

detail="of the last $total runs"
if [ "$running" -gt 0 ]; then
  detail="$detail · $running running"
fi
ssp metric set "$id" --label "CI · $branch" --value "$failed" --unit failed \
  --detail "$detail" --ttl 900
