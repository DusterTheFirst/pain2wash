FROM lukemathwalker/cargo-chef:latest-rust-1 AS chef

# Apt dependencies
RUN apt update && apt install -y lld

WORKDIR /app

FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS builder
COPY --from=planner /app/recipe.json recipe.json
# Build dependencies - this is the caching Docker layer!
RUN cargo chef cook --release --recipe-path recipe.json

# Build application
COPY . .
RUN set -eux; \
    # Make Git happy (fly.toml does not get copied when running `fly deploy`)
    git restore fly.toml; \
    cargo build --release; \
    objcopy --compress-debug-sections target/release/pain2wash /app/pain2wash

FROM gcr.io/distroless/cc AS runtime

COPY --from=builder \
    /app/pain2wash \
    /pain2wash

CMD [ "/pain2wash" ]