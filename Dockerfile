FROM rust:1.99-trixie AS build
WORKDIR /build
ENV CARGO_BUILD_JOBS=2
COPY Cargo.toml Cargo.lock ./
COPY build.rs ./
COPY .cargo ./.cargo
COPY src ./src
COPY migrations ./migrations
COPY templates ./templates
COPY assets ./assets
RUN cargo build --locked --release

FROM debian:trixie-slim@sha256:a29215f6a35e51e22adffa17f89e9d2ef06214e64a2bad10d765c46aea49f11f
RUN apt-get update \
    && apt-get install -y --no-install-recommends ca-certificates ffmpeg libvips-tools curl \
    && rm -rf /var/lib/apt/lists/* \
    && useradd --uid 10001 --no-create-home booru
COPY --from=build /build/target/release/booru-thingy /usr/local/bin/booru-thingy
USER 10001:10001
ENV LISTEN_ADDR=0.0.0.0:8080 VIPS_CONCURRENCY=1
EXPOSE 8080
HEALTHCHECK --interval=30s --timeout=6s --start-period=15s \
    CMD curl --fail --silent http://127.0.0.1:8080/healthz || exit 1
ENTRYPOINT ["booru-thingy"]
CMD ["run"]
