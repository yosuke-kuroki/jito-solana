#!/usr/bin/env bash
#
# Reports cpu and ram usage statistics
#
set -e

[[ $(uname) == Linux ]] || exit 0

# need to cd like this to avoid #SC1091
cd "$(dirname "$0")/.."
source scripts/configure-metrics.sh

while true; do
  # collect the total cpu usage by subtracting idle usage from 100%
  cpu_usage=$(top -bn1 | grep '%Cpu(s):' | sed "s/.*, *\([0-9.]*\)%* id.*/\1/" | awk '{print 100 - $1}')
  # collect the total ram usage by dividing used memory / total memory
  ram_total_and_usage=$(top -bn1 | grep 'MiB Mem'| sed "s/.*: *\([0-9.]*\)%* total.*, *\([0-9.]*\)%* used.*/\1 \2/")
  read -r total used <<< "$ram_total_and_usage"
  ram_usage=$(awk "BEGIN {print $used / $total * 100}")

  report="cpu_usage=$cpu_usage,ram_usage=$ram_usage"
  ./scripts/metrics-write-datapoint.sh "system-stats,hostname=$HOSTNAME $report"
  sleep 1
done
