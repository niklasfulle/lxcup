FROM rust:1.85-bookworm AS build
WORKDIR /src
COPY . .
RUN cargo build --release -p lxcup-server

FROM debian:bookworm-slim
RUN apt-get update \
    && apt-get install --no-install-recommends --yes ca-certificates wget \
    && install -d /usr/share/postgresql-common/pgdg \
    && wget --quiet --output-document=/usr/share/postgresql-common/pgdg/apt.postgresql.org.asc https://www.postgresql.org/media/keys/ACCC4CF8.asc \
    && printf 'Types: deb\nURIs: https://apt.postgresql.org/pub/repos/apt\nSuites: bookworm-pgdg\nComponents: main\nSigned-By: /usr/share/postgresql-common/pgdg/apt.postgresql.org.asc\n' > /etc/apt/sources.list.d/pgdg.sources \
    && apt-get update \
    && apt-get install --no-install-recommends --yes postgresql-client-17 \
    && rm -rf /var/lib/apt/lists/*
RUN useradd --system --uid 10001 lxcup
COPY --from=build /src/target/release/lxcup-server /usr/local/bin/lxcup-server
COPY --chmod=755 deploy/server-entrypoint.sh /usr/local/bin/server-entrypoint.sh
EXPOSE 8080
ENTRYPOINT ["/usr/local/bin/server-entrypoint.sh"]
