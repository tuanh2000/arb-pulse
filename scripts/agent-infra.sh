#!/usr/bin/env bash
# Agent: infra — PostgreSQL + Redis via docker-compose.
# This must be the first agent started; everything else depends on it.
source "$(dirname "$0")/common.sh"
NAME="infra"

cmd="${1:-start}"

case "$cmd" in

start)
    echo "[$NAME] Starting docker services..."
    cd "$ROOT"
    docker-compose up -d

    echo "[$NAME] Waiting for PostgreSQL..."
    n=0
    until docker-compose exec -T postgres pg_isready -U arbpulse -d arbpulse >/dev/null 2>&1; do
        sleep 2; n=$((n + 2))
        if (( n > 60 )); then
            echo "[$NAME] ERROR: PostgreSQL not ready after 60s" >&2
            exit 1
        fi
    done
    echo "[$NAME] PostgreSQL ready"

    echo "[$NAME] Waiting for Redis..."
    n=0
    until docker-compose exec -T redis redis-cli ping >/dev/null 2>&1; do
        sleep 2; n=$((n + 2))
        if (( n > 60 )); then
            echo "[$NAME] ERROR: Redis not ready after 60s" >&2
            exit 1
        fi
    done
    echo "[$NAME] Redis ready"
    ;;

stop)
    echo "[$NAME] Stopping docker services..."
    cd "$ROOT"
    docker-compose down
    ;;

status)
    cd "$ROOT"
    docker-compose ps
    ;;

*)
    echo "Usage: $0 {start|stop|status}" >&2
    exit 1
    ;;
esac
