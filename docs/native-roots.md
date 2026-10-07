# Trusting a private CA in buzz-sprig

## The problem

`buzz-acp` opens its relay connection with `tokio-tungstenite` built with the
`rustls-tls-webpki-roots` feature (workspace `Cargo.toml` in block/buzz). That
compiles Mozilla's public root list into the binary, so the connection ignores
every runtime trust setting. Measured against a step-ca-issued relay
certificate with the stock image (`buzz-sprig@sha256:65b061ae…`, block/buzz
`da651ad`):

| Trust setup | private-CA relay | public relay |
|---|---|---|
| none | UnknownIssuer | ok |
| `SSL_CERT_FILE` = private root | UnknownIssuer | ok |
| `SSL_CERT_DIR` incl. private root | UnknownIssuer | ok |
| private root mounted over the system bundle | UnknownIssuer | ok |

HTTPS calls made through `reqwest` (model APIs) already honor the system store;
only the relay socket does not. Any self-hosted relay behind an internal CA —
the sovereign-relay story in `VISION_SOVEREIGN.md` — cannot host remote agents.

## The change

```diff
-tokio-tungstenite = { version = "0.29", features = ["rustls-tls-webpki-roots"] }
+tokio-tungstenite = { version = "0.29", features = ["rustls-tls-native-roots"] }
```

With native roots (`rustls-native-certs`), the same matrix becomes:

| Trust setup | private-CA relay | public relay |
|---|---|---|
| none | UnknownIssuer | ok |
| `SSL_CERT_FILE` = private root | ok | UnknownIssuer (file replaces the store) |
| `SSL_CERT_DIR=/etc/ssl/certs:/etc/buzz-ca` | ok | ok |

`SSL_CERT_DIR` with both directories is what `buzz-backend-docker` sets.
Behavior with no variables set is unchanged for public relays, because the
image ships Alpine's `ca-certificates` bundle.

## Building the image

This repo publishes it: run the `sprig-image` workflow with a block/buzz commit,
then pin the printed digest as `DEFAULT_IMAGE` in `src/config.rs`. By hand:

```sh
git clone https://github.com/block/buzz && cd buzz
git checkout <commit>
sed -i 's/"rustls-tls-webpki-roots"/"rustls-tls-native-roots"/' Cargo.toml
sed -i 's/cargo build --locked --profile sprig/cargo build --profile sprig/' Dockerfile.sprig
docker build -f Dockerfile.sprig -t buzz-sprig-nativeroots:<commit> .
```

`--locked` is dropped because the feature switch pulls in `rustls-native-certs`.
Upstreaming the change would regenerate `Cargo.lock` and keep `--locked`.
