#!/usr/bin/env bash

me="$0"
log_file=

teardown () {
    if [ "$log_file" ]; then
        docker compose stop
        docker compose logs --timestamps > "$log_file"
    fi
    docker compose down --volumes
}

die () {
    echo "$1" >&2
    teardown
    exit 1
}

while :; do
    case $1 in
        -h|--help)
            echo "Usage: $0 [--log-file path]"
            exit 2
            ;;
        --log-file)
            if [ "$2" ]; then
                if touch "$2"; then
                    log_file=$2
                    shift
                else
                    die "log file error"
                fi
            else
                die '"--log-file" requires a non-empty option argument'
            fi
            ;;
        *)
            break
    esac
done

set -eux

# Build services images
docker compose build

# Initialize MongoDB
docker compose up -d --quiet-pull mongodb
docker compose exec mongodb mongosh --norc --quiet --eval "rs.initiate()"

# Start pushing data to MongoDB
docker compose exec -d mongodb mongosh --norc /usr/src/push-data.mongodb

# Run tests
if ! docker compose up client --exit-code-from client --no-log-prefix --quiet-pull; then
    die "$me: tests failure"
fi

echo "$me: success"
teardown
