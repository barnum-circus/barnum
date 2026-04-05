#!/bin/bash
# Run tests quietly — only print output on failure.

fail=0

pnpm_output=$(pnpm -r test 2>&1)
if [ $? -ne 0 ]; then
  echo "$pnpm_output"
  fail=1
fi

cargo_output=$(cargo test --workspace 2>&1)
if [ $? -ne 0 ]; then
  echo "$cargo_output"
  fail=1
fi

if [ $fail -eq 0 ]; then
  echo "All tests passed."
fi

exit $fail
