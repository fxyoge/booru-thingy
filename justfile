@default:
  just --list

build:
  cargo build --workspace --locked

test:
  cargo test --workspace --locked

lint:
  cargo fmt --check
  cargo clippy --workspace --all-targets --locked -- -D warnings

# Requires a running disposable target, test dependencies, Chromium, and ffmpeg.
e2e:
    python -m pytest tests/e2e --tracing=retain-on-failure --screenshot=only-on-failure --output=test-results/browser --junitxml=test-results/e2e.xml

# Requires pytest-playwright and Chromium; no running services required.
frontend-test:
    python -m pytest tests/frontend

generate-sql:
  test "$(sqlc version)" = "v1.30.0"
  sqlc generate
  rustfmt --edition 2024 src/db/generated.rs

check-sql: generate-sql
  git diff --exit-code -- src/db/generated.rs

run +ARGS:
  cargo run -- {{ARGS}}
