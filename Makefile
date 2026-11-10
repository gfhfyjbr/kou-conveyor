.PHONY: build build-runner build-tui build-web test check notices

build: build-tui build-web

build-runner:
	go build -trimpath -o bin/kou-conveyor-runner ./cmd/kou-conveyor-runner

test:
	go test -race ./...
	# The tests that run the accounts gateway (CLIProxyAPI) skip under the race
	# detector, which reports a race inside it; they run here without it.
	go test -count=1 -run 'TestHost$$|TestAccountsGateway$$' ./cmd/internal/accounts ./cmd/kou-conveyor-web

check:
	go vet ./...
	@test -z "$$(gofmt -l cmd harness internal)" || { gofmt -l cmd harness internal; exit 1; }

# The cockpits launch the runner beside them, so it is rebuilt with them and
# the two never come from different sources.
build-tui: build-runner
	go build -trimpath -o bin/kou-conveyor-tui ./cmd/kou-conveyor-tui

# The browser cockpit follows the checkout it was built from, live. Its
# canvases' nodes call it with kou-canvas, built beside it.
build-web: build-runner
	go build -trimpath -ldflags "-X 'main.builtFrom=$(CURDIR)'" -o bin/kou-conveyor-web ./cmd/kou-conveyor-web
	go build -trimpath -o bin/kou-conveyor-canvas ./cmd/kou-conveyor-canvas

# The licenses of the third-party code the binaries are built from, which go
# wherever the binaries are passed on to: Go's, the modules', and those of the
# fonts and the script the browser cockpit embeds.
notices:
	@mkdir -p bin
	scripts/third-party-notices.sh ./cmd/kou-conveyor-runner ./cmd/kou-conveyor-tui ./cmd/kou-conveyor-web ./cmd/kou-conveyor-canvas > bin/THIRD_PARTY_NOTICES.txt.new
	mv bin/THIRD_PARTY_NOTICES.txt.new bin/THIRD_PARTY_NOTICES.txt

.PHONY: rust-check rust-test rust-build rust-web rust-compat

rust-check:
	cargo fmt --all -- --check
	cargo clippy --locked --workspace --all-targets -- -D warnings
	python3 compat/inventory.py --check

rust-test:
	cargo test --locked --workspace

rust-build:
	cargo build --locked --release --workspace --bins

rust-web:
	python3 compat/build_web.py

rust-compat:
	python3 compat/store_parity.py
	python3 compat/writer_parity.py
	python3 compat/codevm_parity.py
	python3 compat/fileops_parity.py
	python3 compat/patch_parity.py
	python3 compat/native_parity.py
	python3 compat/provider_parity.py
	python3 compat/anthropic_parity.py
	python3 compat/context_parity.py
	python3 compat/replay_parity.py
	python3 compat/loop_parity.py
	python3 compat/operation_parity.py
	python3 compat/capture_parity.py
	python3 compat/shell_parity.py
