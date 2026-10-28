RS_DIR := ./crates

.PHONY: daemon-run daemon-run-test daemon-run-test-live daemon-run-prod daemon-test-status daemon-prod-status daemon-build daemon-test daemon-contract-test

daemon-run: daemon-run-test

daemon-run-test:
	./scripts/run-daemon.sh test

daemon-run-test-live:
	KURA_CONNECTORS_DISCORD_ENABLED=true ./scripts/run-daemon.sh test

daemon-run-prod:
	./scripts/run-daemon.sh prod

daemon-test-status:
	./scripts/check-daemon-health.sh http://127.0.0.1:19192/healthz

daemon-prod-status:
	./scripts/check-daemon-health.sh http://127.0.0.1:19191/healthz

daemon-build:
	cd $(RS_DIR) && cargo build --release -p kura-cli

daemon-test:
	./scripts/cargo-test-isolated.sh --workspace

daemon-contract-test:
	./scripts/cargo-test-isolated.sh -p kura-contracts

.PHONY: rs-build rs-test rs-clippy

rs-build:
	cd $(RS_DIR) && cargo build --workspace

rs-test:
	./scripts/cargo-test-isolated.sh --workspace

rs-clippy:
	cd $(RS_DIR) && cargo clippy --workspace --all-targets
