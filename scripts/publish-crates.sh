#!/bin/bash
set -e

echo "Publishing gsd_macro..."
cargo publish -p gsd_macro

echo "Waiting for crates.io to index gsd_macro..."
sleep 30

echo "Publishing gsd_task_queue..."
cargo publish -p gsd_task_queue

echo "Publishing gsd_multiplexer..."
cargo publish -p gsd_multiplexer

echo "Done!"
