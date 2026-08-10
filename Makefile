# wireless-programmer — build / release helpers

TARGET_MUSL ?= aarch64-unknown-linux-musl
CARGO ?= cargo
RUSTUP_TOOLCHAIN ?= stable
export RUSTUP_TOOLCHAIN

.PHONY: all build release release-musl check test test-release-assertions clean fmt clippy

all: build

build:
	$(CARGO) build --workspace

release:
	$(CARGO) build --workspace --release

# Static musl binary (default: aarch64). Override: make release-musl TARGET_MUSL=x86_64-unknown-linux-musl
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
