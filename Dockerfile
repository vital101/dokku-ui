FROM rust:1-bookworm AS chef
RUN cargo install cargo-chef --locked

FROM chef AS plan
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo chef prepare --recipe-path recipe.json

FROM chef AS build
WORKDIR /app
COPY --from=plan /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json
COPY . .
RUN cargo build --release --bin dokku-ui

ARG TAILWIND_VERSION=v4.3.3
RUN set -eux; \
    case "$(uname -m)" in \
      aarch64) TAILWIND_ARCH=linux-arm64 ;; \
      x86_64) TAILWIND_ARCH=linux-x64 ;; \
      *) echo "unsupported architecture"; exit 1 ;; \
    esac; \
    curl -fsSL -o /usr/local/bin/tailwindcss "https://github.com/tailwindlabs/tailwindcss/releases/download/${TAILWIND_VERSION}/tailwindcss-${TAILWIND_ARCH}"; \
    chmod +x /usr/local/bin/tailwindcss
RUN tailwindcss -i assets/input.css -o static/css/app.css --minify

FROM debian:bookworm-slim AS runtime
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates libgcc-s1 \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --create-home --uid 1000 dokku-ui
WORKDIR /app
COPY --from=build /app/target/release/dokku-ui /app/dokku-ui
COPY --from=build /app/static/css/app.css /app/static/css/app.css
USER dokku-ui
ENV PORT=8080
EXPOSE 8080
ENTRYPOINT ["/app/dokku-ui"]