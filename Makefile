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
