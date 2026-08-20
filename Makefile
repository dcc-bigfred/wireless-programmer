# wireless-programmer — build / release helpers

TARGET_MUSL ?= aarch64-unknown-linux-musl
CARGO ?= cargo
RUSTUP_TOOLCHAIN ?= stable
export RUSTUP_TOOLCHAIN

# Optional wireless iface for `make dev` (e.g. INTERFACE=wlan0).
INTERFACE ?=

.PHONY: all build release release-musl check test test-release-assertions clean fmt clippy dev

all: build

build:
	$(CARGO) build --workspace

# Build and run the daemon in the foreground (local development).
# Override iface: make dev INTERFACE=wlp2s0
# Override data root / socket: DATA_DIR=/tmp/wp-dev make dev
dev:
	$(CARGO) run -p wireless-programmer -- daemon --verbose \
		$(if $(INTERFACE),--interface $(INTERFACE),)

release:
	$(CARGO) build --workspace --release

# Static musl binary (default: aarch64). Override: make release-musl TARGET_MUSL=x86_64-unknown-linux-musl
# Optional: WIRELESS_PROGRAMMER_GIT_COMMIT / WIRELESS_PROGRAMMER_BUILD_TIME for version metadata.
release-musl:
	RUSTFLAGS='-C target-feature=+crt-static' \
		$(CARGO) build --workspace --release --target $(TARGET_MUSL)
	@mkdir -p dist
	@case "$(TARGET_MUSL)" in \
		aarch64-*) dist_name=wireless-programmer-linux-arm64 ;; \
		x86_64-*)  dist_name=wireless-programmer-linux-amd64 ;; \
		*)         dist_name=wireless-programmer-$(TARGET_MUSL) ;; \
	esac; \
	cp -f target/$(TARGET_MUSL)/release/wireless-programmer "dist/$${dist_name}"; \
	echo "wrote dist/$${dist_name}"

check:
	$(CARGO) check --workspace

test:
	$(CARGO) test --workspace --locked

test-release-assertions:
	$(CARGO) test --workspace --locked --profile release-assertions

fmt:
	$(CARGO) fmt --all

clippy:
	$(CARGO) clippy --workspace --all-targets --locked -- -D warnings

clean:
	$(CARGO) clean
	rm -rf dist
