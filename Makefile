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

## parity: run the original Go suite, unmodified, against this port
##
## Assembles a scratch package from the cgo-free adapter and the pinned test
## files. The pinned files are copied rather than kept alongside the adapter so
## that exactly one copy of them exists in the repository -- the one
## verify-hashes checks.
.PHONY: parity
parity: verify-hashes
	$(CARGO) build --release -p blackfriday-harness
	rm -rf target/parity && mkdir -p target/parity
	cp adapter/go.mod adapter/blackfriday.go target/parity/
	cp tests/original/*_test.go target/parity/
	cp -r tests/original/testdata target/parity/
	cd target/parity && BF_SERVE=../release/bf-serve go test -v ./...

## check: everything CI would run
check: fmt lint test verify-hashes

## clean: remove build output
clean:
	$(CARGO) clean
