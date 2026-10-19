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
# Rootful docker runs the build containers as a real root; they then hand
# what they wrote back to you. Podman and rootless docker already map the
# container's root to you, so nothing is changed there.
CONTAINER_OWNER ?= $(if $(findstring podman,$(CONTAINER)),,$(shell $(CONTAINER) info --format '{{.SecurityOptions}}' 2>/dev/null | grep -q rootless || echo "$$(id -u):$$(id -g)"))
WINDOWS_BUILD_DIR := target/windows-cross
WINDOWS_DIST_DIR := $(DIST_DIR)/windows

# Linux packages. Both bundle the optimized and compatible builds and pick one
# at launch. The Flatpak builds in the flathub-infra image for its runtime.
APPIMAGE_DIST_DIR := $(DIST_DIR)/appimage
FLATPAK_DIST_DIR := $(DIST_DIR)/flatpak
FLATPAK_BUILD_DIR := target/flatpak
FLATPAK_IMAGE ?= ghcr.io/flathub-infra/flatpak-github-actions:freedesktop-26.08

.PHONY: build run release release-optimized release-compatible dist appimage flatpak windows-installer bump release-notes fmt fmt-check clippy test audit deny coverage qa clean

build:
	$(CARGO) build

run:
	$(CARGO) run

release: release-optimized release-compatible

# AVX2/FMA build (x86-64-v3). Umbra briefings run 2-3x faster than the
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

# Nomadeer-<version>-x86_64.AppImage, plus the .zsync file AppImage updaters
# use when zsyncmake (package zsync) is installed. The binaries need the glibc
# of the machine that built them; the release workflow builds on Ubuntu 22.04.
appimage: release
	packaging/linux/appimage/build-appimage.sh $(VERSION) \
		$(OPTIMIZED_TARGET_DIR)/release/nomadeer \
		$(COMPATIBLE_TARGET_DIR)/release/nomadeer \
		$(APPIMAGE_DIST_DIR)

# Nomadeer-<version>-x86_64.flatpak. flatpak-builder sandboxes its build steps
# with bubblewrap, which needs a privileged container.
flatpak: release
	mkdir -p $(FLATPAK_BUILD_DIR) $(FLATPAK_DIST_DIR)
	$(CONTAINER) run --rm --privileged --security-opt label=disable \
		-v "$(CURDIR):/src:ro" \
		-v "$(CURDIR)/$(FLATPAK_BUILD_DIR):/build" \
		-v "$(CURDIR)/$(FLATPAK_DIST_DIR):/dist" \
		-e FLATPAK_WORK_DIR=/build -e HOST_OWNER=$(CONTAINER_OWNER) \
		$(FLATPAK_IMAGE) sh /src/packaging/linux/flatpak/build-flatpak.sh $(VERSION) /dist

# Nomadeer-<version>-Setup.exe and the portable zip, cross-built from Linux.
# Volumes use label=disable instead of :z so your files are never relabelled.
windows-installer:
	mkdir -p $(WINDOWS_BUILD_DIR) $(WINDOWS_DIST_DIR)
	$(CONTAINER) run --rm --security-opt label=disable \
		-v "$(CURDIR):/src:ro" \
		-v "$(CURDIR)/$(WINDOWS_BUILD_DIR):/build" \
		-v "$(CURDIR)/$(WINDOWS_DIST_DIR):/dist" \
		-e HOST_OWNER=$(CONTAINER_OWNER) \
		$(WINDOWS_RUST_IMAGE) sh /src/packaging/windows/build-cross.sh $(VERSION)
	$(CONTAINER) run --rm --security-opt label=disable $(WINDOWS_INNOSETUP_USER) \
		-v "$(CURDIR):/work:ro" \
		-v "$(CURDIR)/$(WINDOWS_BUILD_DIR)/stage:/stage:ro" \
		-v "$(CURDIR)/$(WINDOWS_DIST_DIR):/dist" \
		$(WINDOWS_INNOSETUP_IMAGE) /Q /DAppVersion=$(VERSION) \
		'/DOptimizedExe=Z:\stage\nomadeer-optimized.exe' \
		'/DCompatibleExe=Z:\stage\nomadeer-compatible.exe' \
		'/DUmbraDefines=Z:\stage\umbra-defines.iss' \
		'/OZ:\dist' packaging/windows/nomadeer.iss
	cd $(WINDOWS_DIST_DIR) && sha256sum Nomadeer-$(VERSION)-Setup.exe > Nomadeer-$(VERSION)-Setup.exe.sha256

# Starts release x.y.z: moves CHANGELOG.md's Unreleased notes into a dated
# section and sets the version in Cargo.toml, Cargo.lock and the metainfo.
# Commit the result, then tag it vx.y.z to build a draft GitHub release.
bump:
	@test "$(origin VERSION)" = "command line" || { echo "usage: make bump VERSION=x.y.z" >&2; exit 2; }
	CARGO=$(CARGO) python3 tools/release.py bump $(VERSION)

# Prints a version's changelog section (default: the current version).
release-notes:
	@python3 tools/release.py notes $(VERSION)

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
