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

# Windows cross-build (make windows-installer). Runs in containers so no
# mingw, wine or Inno Setup is needed on the host.
CONTAINER ?= $(shell command -v podman 2>/dev/null || command -v podman-remote 2>/dev/null || echo docker)
WINDOWS_RUST_IMAGE ?= docker.io/library/rust:1.99-slim-bookworm
WINDOWS_INNOSETUP_IMAGE ?= docker.io/amake/innosetup:innosetup6-bookworm@sha256:dfe43a20fc813b5dae219a1d77f5f5a39b571ca178a7f36cd196805a13252246
# The Inno Setup image runs wine as uid 1000 (xclient, gid 101). Podman maps
# the invoking user onto it so the installer lands in dist/ owned by you.
ifneq (,$(findstring podman,$(CONTAINER)))
WINDOWS_INNOSETUP_USER ?= --userns=keep-id:uid=1000,gid=101
endif
WINDOWS_BUILD_DIR := target/windows-cross
WINDOWS_DIST_DIR := $(DIST_DIR)/windows

.PHONY: build run release release-optimized release-compatible dist windows-installer fmt fmt-check clippy test audit deny coverage qa clean

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

# Nomadeer-<version>-Setup.exe and the portable zip, cross-built from Linux.
# Volumes use label=disable instead of :z so your files are never relabelled.
windows-installer:
	mkdir -p $(WINDOWS_BUILD_DIR) $(WINDOWS_DIST_DIR)
	$(CONTAINER) run --rm --security-opt label=disable \
		-v "$(CURDIR):/src:ro" \
		-v "$(CURDIR)/$(WINDOWS_BUILD_DIR):/build" \
		-v "$(CURDIR)/$(WINDOWS_DIST_DIR):/dist" \
		$(WINDOWS_RUST_IMAGE) sh /src/packaging/windows/build-cross.sh $(VERSION)
	$(CONTAINER) run --rm --security-opt label=disable $(WINDOWS_INNOSETUP_USER) \
		-v "$(CURDIR):/work:ro" \
		-v "$(CURDIR)/$(WINDOWS_BUILD_DIR)/stage:/stage:ro" \
		-v "$(CURDIR)/$(WINDOWS_DIST_DIR):/dist" \
		$(WINDOWS_INNOSETUP_IMAGE) /Q /DAppVersion=$(VERSION) \
		'/DOptimizedExe=Z:\stage\nomadeer-optimized.exe' \
		'/DCompatibleExe=Z:\stage\nomadeer-compatible.exe' \
		'/OZ:\dist' packaging/windows/nomadeer.iss
	cd $(WINDOWS_DIST_DIR) && sha256sum Nomadeer-$(VERSION)-Setup.exe > Nomadeer-$(VERSION)-Setup.exe.sha256

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
