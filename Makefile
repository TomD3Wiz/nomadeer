ifneq (,$(wildcard /opt/rust/cargo/bin/cargo))
export CARGO_HOME ?= /opt/rust/cargo
export RUSTUP_HOME ?= /opt/rust/rustup
CARGO ?= /opt/rust/cargo/bin/cargo
else
CARGO ?= cargo
endif

VERSION := $(shell sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -n 1)
DIST_DIR := dist
OPTIMIZED_TARGET_DIR := target/optimized
COMPATIBLE_TARGET_DIR := target/compatible

.PHONY: build run release release-optimized release-compatible dist fmt fmt-check clippy test audit deny coverage qa clean

build:
	$(CARGO) build

run:
	$(CARGO) run

release: release-optimized release-compatible

# AVX2/FMA build (x86-64-v3). Umbra briefings run about 3x faster than the
# compatible build, but the binary will not start on CPUs without AVX2.
release-optimized:
	RUSTFLAGS="-C target-cpu=x86-64-v3" $(CARGO) build --release --target-dir $(OPTIMIZED_TARGET_DIR)

# Baseline x86-64 build for older CPUs.
release-compatible:
	$(CARGO) build --release --target-dir $(COMPATIBLE_TARGET_DIR)

dist: release
	rm -rf $(DIST_DIR)
	for variant in optimized compatible; do \
		name=nomadeer-$(VERSION)-linux-x86_64-$$variant; \
		mkdir -p $(DIST_DIR)/$$name; \
		cp target/$$variant/release/nomadeer README.md $(DIST_DIR)/$$name/; \
		tar -C $(DIST_DIR) -czf $(DIST_DIR)/$$name.tar.gz $$name; \
		(cd $(DIST_DIR) && sha256sum $$name.tar.gz > $$name.tar.gz.sha256); \
	done

fmt:
	$(CARGO) fmt

fmt-check:
	$(CARGO) fmt --check

clippy:
	$(CARGO) clippy -- -D warnings

test:
	$(CARGO) test

audit:
	$(CARGO) audit \
		--ignore RUSTSEC-2026-0367 \
		--ignore RUSTSEC-2026-0368 \
		--ignore RUSTSEC-2026-0369

deny:
	$(CARGO) deny check

coverage:
	$(CARGO) llvm-cov

qa: fmt-check clippy test audit deny

clean:
	$(CARGO) clean
