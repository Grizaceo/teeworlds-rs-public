# teeworlds-rs — build multi-stage: wasm cliente + server autocontenido
FROM rust:1.90-bookworm AS build
WORKDIR /app
RUN rustup target add wasm32-unknown-unknown
COPY . .
RUN cargo build --release -p client --target wasm32-unknown-unknown \
 && cp target/wasm32-unknown-unknown/release/teeworlds-rs.wasm teeworlds_rs.wasm \
 && cargo build --release -p server

FROM debian:bookworm-slim
RUN useradd -r -u 10001 app
COPY --from=build /app/target/release/teeworlds-rs-server /usr/local/bin/server
# Pesos de bots entrenados (opcional: sin BOTS>0 ni se leen)
COPY --from=build /app/bots /app/bots
# Mapas ASCII para MODE=race (puzzle1.txt + resto de maps/)
COPY --from=build /app/maps /app/maps
WORKDIR /app
USER app
ENV PORT=8303
EXPOSE 8303
CMD ["teeworlds-rs-server"]
