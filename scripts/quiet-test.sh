#!/bin/bash
# Run tests quietly — only print output on failure.

output=$(pnpm -r test 2>&1)
status=$?
if [ $status -ne 0 ]; then
  echo "$output"
fi
exit $status
