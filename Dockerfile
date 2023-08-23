FROM rust as builder
WORKDIR /app
COPY Cargo.toml Cargo.lock ./
RUN mkdir src/
RUN echo "fn main() { }" > src/main.rs
RUN cargo build --release
COPY src ./src
RUN cargo build --release

FROM gcr.io/distroless/cc AS runtime
WORKDIR /app
COPY --from=builder /app/target/release/calabi /calabi
ENTRYPOINT ["/calabi"]
