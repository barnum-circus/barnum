#!/bin/bash
# Watch a specific GitHub Actions job within a run.
#
# Usage: ./watch-ci-job.sh <run-id> <job-name>
#
# Polls the job status every 10 seconds until completion.
# Exits 0 on success, 1 on failure.

set -e

if [[ $# -lt 2 ]]; then
    echo "Usage: $0 <run-id> <job-name>"
    exit 1
fi

RUN_ID="$1"
JOB_NAME="$2"

echo "Watching job '$JOB_NAME' in run $RUN_ID..."

while true; do
    result=$(gh run view "$RUN_ID" --json jobs --jq ".jobs[] | select(.name == \"$JOB_NAME\") | {status, conclusion}")

    status=$(echo "$result" | jq -r '.status')
    conclusion=$(echo "$result" | jq -r '.conclusion')

    echo "[$(date '+%H:%M:%S')] Status: $status, Conclusion: $conclusion"

    if [[ "$status" == "completed" ]]; then
        echo "Job finished with conclusion: $conclusion"
        if [[ "$conclusion" == "success" ]]; then
            exit 0
        else
            exit 1
        fi
    fi

    sleep 10
done
