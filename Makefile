# arb-pulse agent team — control plane
# ─────────────────────────────────────────────────────────────────────────────
#  make up                        build & run the Rust orchestrator (recommended)
#  make up-scripts                legacy shell-script startup (no auto-restart)
#  make down                      stop all agents + infrastructure
#  make status                    health overview of every component
#
#  make infra                     start postgres + redis (docker-compose)
#  make pool-registry             start pool-registry agent (port 3001)
#  make listener                  start listener agent      (port 3000)
#  make opportunity-finder        start opportunity-finder agent
#  make broadcaster               start transaction-broadcaster (needs PRIVATE_KEY)
#  make deploy-contract           deploy ArbExecutor to PulseChain (needs PRIVATE_KEY)
#
#  make stop-<agent>              e.g. make stop-listener
#  make restart-<agent>           e.g. make restart-pool-registry
#  make logs-<agent>              tail live logs,  e.g. make logs-opportunity-finder
#
#  make build                     cargo build --workspace (all crates)
#  make build-release             cargo build --release --workspace
# ─────────────────────────────────────────────────────────────────────────────

SHELL := /usr/bin/env bash
.PHONY: up up-scripts down status build build-release orchestrator \
        infra pool-registry listener opportunity-finder broadcaster deploy-contract \
        stop-infra stop-pool-registry stop-listener stop-opportunity-finder stop-broadcaster \
        restart-pool-registry restart-listener restart-opportunity-finder restart-broadcaster

# ── Full stack ────────────────────────────────────────────────────────────────

# Rust orchestrator: starts infra then supervises all agents with auto-restart + DB logging.
up: infra
	@cargo build -p orchestrator -p pool-registry -p listener -p opportunity-finder -p transaction-broadcaster 2>&1 | tail -3
	@./target/debug/orchestrator

# Legacy shell-script approach (no auto-restart, no DB event logging).
up-scripts:
	@scripts/start-all.sh

down:
	@scripts/stop-all.sh

status:
	@scripts/status.sh

# ── Build ─────────────────────────────────────────────────────────────────────

build:
	cargo build --workspace

build-release:
	cargo build --release --workspace

orchestrator:
	cargo build -p orchestrator && ./target/debug/orchestrator

# ── Individual agents — start ─────────────────────────────────────────────────

infra:
	@scripts/agent-infra.sh start

pool-registry:
	@scripts/agent-pool-registry.sh start

listener:
	@scripts/agent-listener.sh start

opportunity-finder:
	@scripts/agent-opportunity-finder.sh start

broadcaster:
	@scripts/agent-broadcaster.sh start

deploy-contract:
	@scripts/agent-contract.sh deploy

# ── Individual agents — stop ──────────────────────────────────────────────────

stop-infra:
	@scripts/agent-infra.sh stop

stop-pool-registry:
	@scripts/agent-pool-registry.sh stop

stop-listener:
	@scripts/agent-listener.sh stop

stop-opportunity-finder:
	@scripts/agent-opportunity-finder.sh stop

stop-broadcaster:
	@scripts/agent-broadcaster.sh stop

# ── Individual agents — restart ───────────────────────────────────────────────

restart-pool-registry:
	@scripts/agent-pool-registry.sh restart

restart-listener:
	@scripts/agent-listener.sh restart

restart-opportunity-finder:
	@scripts/agent-opportunity-finder.sh restart

restart-broadcaster:
	@scripts/agent-broadcaster.sh restart

# ── Log tailing ───────────────────────────────────────────────────────────────

logs-%:
	@tail -f logs/$*.log
