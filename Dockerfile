# ============================================================================
# Stage 1: Build
# ============================================================================
FROM rust:1.84 AS builder

WORKDIR /app

# Install build dependencies
RUN apt-get update && apt-get install -y \
    pkg-config \
    libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Copy manifests first for dependency caching
COPY Cargo.toml Cargo.lock ./

# Create dummy main.rs to build dependencies
RUN mkdir -p src && \
    echo 'fn main() { println!("dummy"); }' > src/main.rs

# Build dependencies only (cached layer)
RUN cargo build --release && \
    rm -rf src target/release/deps/watchtower*

# Copy actual source code
COPY src ./src
COPY migrations ./migrations

# Build the actual application
RUN cargo build --release

# ============================================================================
# Stage 2: Runtime
# ============================================================================
FROM debian:bookworm-slim

# Install runtime dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates \
    libssl3 \
    libsqlite3-0 \
    && rm -rf /var/lib/apt/lists/*

# Create non-root user
RUN useradd -m -s /bin/bash watchtower

WORKDIR /app

# Copy binary from builder
COPY --from=builder /app/target/release/watchtower /usr/local/bin/watchtower
RUN chmod +x /usr/local/bin/watchtower

# Copy templates and static files
COPY templates ./templates
COPY static ./static
COPY migrations ./migrations

# Create data directory
RUN mkdir -p /data && chown -R watchtower:watchtower /app /data

# Switch to non-root user
USER watchtower

# Expose port
EXPOSE 3002

# Environment defaults
ENV PORT=3002 \
    DATABASE_URL=sqlite:/data/watchtower.db \
    RUST_LOG=info

# Health check
HEALTHCHECK --interval=30s --timeout=10s --start-period=5s --retries=3 \
    CMD wget --no-verbose --tries=1 --spider http://localhost:3002/health || exit 1

CMD ["watchtower"]
