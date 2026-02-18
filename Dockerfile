FROM rust:1.88-bookworm AS builder
WORKDIR /app

# Cache dependencies
COPY Cargo.toml Cargo.lock ./
RUN mkdir src && echo "fn main() {}" > src/main.rs && cargo build --release && rm -rf src

# Build real app
COPY src/ src/
COPY templates/ templates/
COPY migrations/ migrations/
RUN touch src/main.rs && cargo build --release

# Runtime
FROM debian:bookworm-slim
RUN apt-get update && apt-get install -y ca-certificates wget && rm -rf /var/lib/apt/lists/*

COPY --from=builder /app/target/release/watchtower /usr/local/bin/
COPY --from=builder /app/templates /app/templates
COPY --from=builder /app/migrations /app/migrations

WORKDIR /app
EXPOSE 3002

HEALTHCHECK --interval=30s --timeout=10s --retries=3 --start-period=10s \
    CMD wget --no-verbose --tries=1 --spider http://localhost:3002/ || exit 1

CMD ["watchtower"]
