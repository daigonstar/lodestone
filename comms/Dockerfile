# Lodestone Comms host. Build: docker build -t lodestone-comms .   (or podman build)
FROM docker.io/library/rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY server ./server
RUN cargo build --release --locked -p lodestone-comms-server

FROM docker.io/library/debian:bookworm-slim
RUN useradd --system --uid 10001 --home /data comms && mkdir -p /data && chown comms /data
COPY --from=build /src/target/release/lodestone-comms-server /usr/local/bin/lodestone-comms-server
USER comms
ENV COMMS_DATA=/data COMMS_BIND=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
CMD ["lodestone-comms-server"]
