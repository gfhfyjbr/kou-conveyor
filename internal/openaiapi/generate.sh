#!/bin/sh

set -eu

spec_file="$(mktemp -t openai-responses.XXXXXX)"
trap 'rm -f "$spec_file"' EXIT

# oapi-codegen prunes webhook schemas but still emits aliases referencing them.
awk '
    /^webhooks:/ { skipping = 1; next }
    skipping && /^[^[:space:]#]/ { skipping = 0 }
    !skipping { print }
' ../../third_party/openai-openapi/openapi.yaml >"$spec_file"

go run github.com/oapi-codegen/oapi-codegen/v2/cmd/oapi-codegen@v2.8.0 \
    -generate types \
    -include-operation-ids createResponse \
    -package openaiapi \
    -o types.gen.go \
    "$spec_file"
