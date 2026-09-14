
help:
	@echo "Usage: make <target>"
	@echo ""
	@echo "  check          CI gate: lint then test (must pass before merging)"
	@echo "  act            run CI workflow locally via act"
	@echo "  lint           cargo fmt --check + clippy -D warnings"
	@echo "  test           all tests: unit + integration"
	@echo "  test-unit      unit tests only (cargo test --lib)"
	@echo "  test-int       integration tests only (cargo test --tests)"
	@echo "  test-nostd     no_std + no-poseidon builds (zkVM guest configurations)"
	@echo "  fix            auto-format + apply safe clippy fixes"
	@echo "  build          cargo build"

# CI target — must pass before merging
check: lint test test-nostd

# Format check + clippy (warnings are errors)
lint:
	cargo fmt --check
	cargo clippy -- -D warnings

# All tests: unit (within modules) + integration (tests/)
test:
	cargo test

# Unit tests only
test-unit:
	cargo test --lib

# Integration tests only
test-integration:
	cargo test --tests

# The feature configurations a zkVM guest builds under. `poseidon` pulls in
# wasmer -> cranelift -> target-lexicon, whose build script rejects custom
# RISC-V target triples, so the guest must compile without it. Warnings are
# errors here: an unused import under one feature set is a real defect.
#
# These are builds, not test runs — the point is that the configurations
# compile at all. `test` already runs the suite under default features.
test-nostd:
	RUSTFLAGS="-D warnings" cargo build -p proveno --no-default-features
	RUSTFLAGS="-D warnings" cargo build -p proveno --no-default-features --features "std"
	cargo test -p proveno --no-default-features --features "std"

# Auto-fix formatting and apply safe clippy suggestions
fix:
	cargo fmt
	cargo clippy --fix --allow-dirty --allow-staged

build:
	cargo build

# Run CI workflow locally via act
act:
	act
