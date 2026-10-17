ifneq (,$(wildcard /opt/rust/cargo/bin/cargo))
export CARGO_HOME ?= /opt/rust/cargo
export RUSTUP_HOME ?= /opt/rust/rustup
CARGO ?= /opt/rust/cargo/bin/cargo
else
CARGO ?= cargo
endif

.PHONY: build run release fmt fmt-check clippy test audit deny coverage qa clean

build:
	$(CARGO) build

run:
	$(CARGO) run

release:
	$(CARGO) build --release

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
