# Multi-stage build for the `crawler` binary.
#
# chef -> planner -> builder -> distroless runtime (D-05).
# The build needs NO live DATABASE_URL: it compiles against the committed
# offline sqlx metadata with SQLX_OFFLINE (D-06, IMAGE-01). The runtime image
# carries only the release binary on a non-root distroless base (D-01/D-07/D-08,
# IMAGE-02). Bases are tag-pinned (no digest pin) per D-10.

# Stage 1: chef — shared base with cargo-chef installed (D-04).
FROM rust:1.94-bookworm AS chef
WORKDIR /app
RUN cargo install cargo-chef

# Stage 2: planner — produce the dependency recipe from the full source tree (D-05).
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# Stage 3: builder — cook dependencies (cached layer for ~200 crates), then
# build the release binary offline against the committed .sqlx/ metadata.
# rustls-only stack (D-02): no system TLS dev packages or apt installs needed.
FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
ENV SQLX_OFFLINE=true
RUN cargo build --release

# Stage 4: runtime — distroless glibc base shipping ca-certificates + a non-root
# user, no shell / package manager (D-01). Copy ONLY the release binary (D-07);
# migrations are embedded at compile time so the SQL dir is not shipped.
FROM gcr.io/distroless/cc-debian12:nonroot
COPY --from=builder /app/target/release/crawler /crawler
# Built-in distroless nonroot user, uid:gid 65532:65532 (D-08).
USER nonroot
# Documentation-only hint for the metrics port; publishes nothing (Phase 7).
EXPOSE 9100
# Runnable-with-args, not self-starting; config / WOT__* wiring is Phase 7.
ENTRYPOINT ["/crawler"]
