# OpenAI OpenAPI specification

This directory vendors the OpenAI OpenAPI specification from
[`openai/openai-openapi`](https://github.com/openai/openai-openapi).

- Revision: `dc708bbe9a149bc35132c567ef3a3fdd7a24ab49`
- Source: `https://raw.githubusercontent.com/openai/openai-openapi/dc708bbe9a149bc35132c567ef3a3fdd7a24ab49/openapi.yaml`
- SHA-256: `ab0c5306e390c64efbf50bbf71f02aa0dad2dafcaa96066a592186daa6103b87`
- Generator: `github.com/oapi-codegen/oapi-codegen/v2/cmd/oapi-codegen@v2.8.0`

Run `go generate ./internal/openaiapi` from the repository root after updating
the pinned specification. Generation selects the `createResponse` operation and
removes the unrelated top-level webhook routes before invoking `oapi-codegen`;
otherwise its operation filter emits request-body aliases for excluded webhook
schemas.
