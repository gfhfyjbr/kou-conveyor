#!/bin/bash
set -euo pipefail
cp /app/model-requests.jsonl /logs/verifier/model-requests.jsonl
test "$(cat /app/answer)" = done
printf '1\n' > /logs/verifier/reward.txt
