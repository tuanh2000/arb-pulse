# arb-pulse agent team — control plane
# ─────────────────────────────────────────────────────────────────────────────
#  make up                        build & run the Rust orchestrator (recommended)
#  make up-scripts                legacy shell-script startup (no auto-restart)
#  make down                      stop all agents + infrastructure
#  make status                    health overview of every component
#
#  make infra                     start postgres + redis (docker-compose)
#  make listener                  start listener agent      (port 3000)
#  make opportunity-finder        start opportunity-finder agent
#  make broadcaster               start transaction-broadcaster (needs PRIVATE_KEY)
#  make deploy-contract           deploy ArbExecutor to PulseChain (needs PRIVATE_KEY)
#
#  Pool-registry (split services):
#  make pool-registry-all         start all three pool-registry services
#  make pool-registry-tvl         seeder + TVL worker          (API :3003, metrics :9107)
#  make pool-registry-price       price oracle                 (API :3002, metrics :9106)
#  make pool-registry-metadata    metadata + FoT/meme screener (API :3001, metrics :9105)
#
#  Web stack:
#  make web                       start arb-api (:4000) + Next.js frontend (:3100)
#  make stop-web                  stop arb-api + frontend
#  make restart-web               restart web stack
#
#  make stop-<agent>              e.g. make stop-listener
#  make restart-<agent>           e.g. make restart-pool-registry-metadata
#  make logs-<agent>              tail live logs, e.g. make logs-pool-registry-tvl
#
#  make build                     cargo build --workspace (all crates)
#  make build-release             cargo build --release --workspace
#
#  make monitoring                start Prometheus + Grafana + exporters (docker)
#  make stop-monitoring           stop the monitoring stack
#                                 Grafana    -> http://localhost:3030 (admin/admin)
#                                 Prometheus -> http://localhost:9090
# ─────────────────────────────────────────────────────────────────────────────

SHELL := /usr/bin/env bash
.PHONY: up up-scripts down status build build-release orchestrator \
        infra listener opportunity-finder broadcaster deploy-contract \
        pool-registry-all pool-registry-tvl pool-registry-price pool-registry-metadata \
        stop-pool-registry-all stop-pool-registry-tvl stop-pool-registry-price stop-pool-registry-metadata \
        restart-pool-registry-all restart-pool-registry-tvl restart-pool-registry-price restart-pool-registry-metadata \
        web stop-web restart-web \
        stop-infra stop-listener stop-opportunity-finder stop-broadcaster \
        restart-listener restart-opportunity-finder restart-broadcaster \
        monitoring stop-monitoring

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

# ── Infrastructure ────────────────────────────────────────────────────────────

infra:
	@scripts/agent-infra.sh start

stop-infra:
	@scripts/agent-infra.sh stop

# ── Pool-registry (split services) ───────────────────────────────────────────

pool-registry-all:
	@scripts/agent-pool-registry-all.sh start

pool-registry-tvl:
	@scripts/agent-pool-registry-tvl.sh start

pool-registry-price:
	@scripts/agent-pool-registry-price.sh start

pool-registry-metadata:
	@scripts/agent-pool-registry-metadata.sh start

stop-pool-registry-all:
	@scripts/agent-pool-registry-all.sh stop

stop-pool-registry-tvl:
	@scripts/agent-pool-registry-tvl.sh stop

stop-pool-registry-price:
	@scripts/agent-pool-registry-price.sh stop

stop-pool-registry-metadata:
	@scripts/agent-pool-registry-metadata.sh stop

restart-pool-registry-all:
	@scripts/agent-pool-registry-all.sh restart

restart-pool-registry-tvl:
	@scripts/agent-pool-registry-tvl.sh restart

restart-pool-registry-price:
	@scripts/agent-pool-registry-price.sh restart

restart-pool-registry-metadata:
	@scripts/agent-pool-registry-metadata.sh restart

# ── Web stack (arb-api + Next.js) ─────────────────────────────────────────────

web:
	@scripts/agent-web.sh start

stop-web:
	@scripts/agent-web.sh stop

restart-web:
	@scripts/agent-web.sh restart

# ── Other agents ──────────────────────────────────────────────────────────────

listener:
	@scripts/agent-listener.sh start

opportunity-finder:
	@scripts/agent-opportunity-finder.sh start

broadcaster:
	@scripts/agent-broadcaster.sh start

deploy-contract:
	@scripts/agent-contract.sh deploy

stop-listener:
	@scripts/agent-listener.sh stop

stop-opportunity-finder:
	@scripts/agent-opportunity-finder.sh stop

stop-broadcaster:
	@scripts/agent-broadcaster.sh stop

restart-listener:
	@scripts/agent-listener.sh restart

restart-opportunity-finder:
	@scripts/agent-opportunity-finder.sh restart

restart-broadcaster:
	@scripts/agent-broadcaster.sh restart

# ── Monitoring stack (Prometheus + Grafana + exporters) ─────────────────────────

monitoring:
	@docker-compose up -d prometheus grafana redis_exporter postgres_exporter
	@echo "Grafana:    http://localhost:3030  (admin/admin)"
	@echo "Prometheus: http://localhost:9090"

stop-monitoring:
	@docker-compose stop prometheus grafana redis_exporter postgres_exporter

# ── Log tailing ───────────────────────────────────────────────────────────────

logs-%:
	@tail -f logs/$*.log
