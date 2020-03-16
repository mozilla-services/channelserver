# Docker 17.05 or higher required for multi-stage builds
FROM rust:1.42.0-stretch as builder
ADD . /app
WORKDIR /app
# Make sure that this matches in .travis.yml
ARG RUST_TOOLCHAIN=stable
RUN \
    apt-get -qq update && \
    \
    rustup default ${RUST_TOOLCHAIN} && \
    cargo --version && \
    rustc --version && \
    mkdir -m 755 bin && \
    cargo build --release && \
    cp /app/target/release/channelserver /app/bin && \
    cp -r /app/channelserver/mmdb /app/mmdb && \
    cp /app/channelserver/version.json /app

FROM debian:stretch-slim
# FROM debian:stretch  # for debugging docker build
MAINTAINER <src+pairsona@jrconlin.com>
RUN \
    groupadd --gid 10001 app && \
    useradd --uid 10001 --gid 10001 --home /app --create-home app && \
    \
    apt-get -qq update && \
    apt-get -qq install -y libssl-dev ca-certificates && \
    update-ca-certificates && \
    rm -rf /var/lib/apt/lists

COPY --from=builder /app/bin /app/bin
COPY --from=builder /app/version.json /app
COPY --from=builder /app/channelserver/mmdb /app/mmdb

WORKDIR /app
USER app

# Channelserver Uses ports: 8000
# override rocket's dev env defaulting to localhost
#ENV ROCKET_ADDRESS 0.0.0.0

CMD ["/app/bin/channelserver"]
