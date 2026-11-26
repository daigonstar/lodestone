# Lodestone Beacon server. Build: docker build -t lodestone-beacon .   (or podman build)
FROM docker.io/library/rust:1-bookworm AS build
WORKDIR /src
COPY Cargo.toml Cargo.lock ./
COPY server ./server
COPY client ./client
COPY app ./app
COPY media ./media
RUN cargo build --release --locked -p lodestone-beacon-server

FROM docker.io/library/debian:bookworm-slim
RUN useradd --system --uid 10001 --home /data beacon && mkdir -p /data && chown beacon /data
COPY --from=build /src/target/release/lodestone-beacon-server /usr/local/bin/lodestone-beacon-server
USER beacon
ENV BEACON_DATA=/data BEACON_BIND=0.0.0.0:8080
VOLUME /data
EXPOSE 8080
CMD ["lodestone-beacon-server"]
