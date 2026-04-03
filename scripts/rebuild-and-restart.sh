#!/usr/bin/env bash
set -euo pipefail

cd /home/ubuntu/kiro-rs

cargo build --release
sudo systemctl restart kiro-rs
sudo systemctl --no-pager --full status kiro-rs
