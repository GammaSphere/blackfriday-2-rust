# blackfriday-rs
#
# `make` builds a runnable artifact. Everything else is optional.
#
# Targets are added as the corresponding part of the port lands, so that no
# target in this file is ever a stub that prints an apology. If it is listed
# here, it works.

CARGO ?= cargo

.DEFAULT_GOAL := build
.PHONY: build test fmt lint verify-hashes check clean

## build: compile the release library
build:
	$(CARGO) build --release

## test: run the port's own test suite
test:
	$(CARGO) test

## fmt: check formatting (does not rewrite)
fmt:
	$(CARGO) fmt --all -- --check

## lint: clippy, warnings are errors
lint:
	$(CARGO) clippy --all-targets -- -D warnings

## verify-hashes: prove the vendored original suite is unmodified
##
## Recomputes every digest in tests/original/SHA256SUMS and then the digest of
## the manifest itself, which is the kickoff hash recorded in .port-mortem.toml.
verify-hashes:
	@cd tests/original && sha256sum -c SHA256SUMS >/dev/null && \
		echo "all 52 files match" && \
		echo -n "manifest: " && sha256sum SHA256SUMS | cut -d' ' -f1

## check: everything CI would run
check: fmt lint test verify-hashes

## clean: remove build output
clean:
	$(CARGO) clean
