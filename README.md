# buzz-backend-docker

Run [Buzz](https://github.com/block/buzz) agents on a Docker host instead of your laptop.

Buzz Desktop can hand an agent to a *backend provider*: any executable named
`buzz-backend-<id>` on its PATH. The provider receives the agent's identity and
launch settings, starts the agent somewhere else, and steps away. From then on
the agent lives on the relay like any other member — status is its presence,
stop is `!shutdown`. Upstream ships a Kubernetes provider. This is the Docker
one: one container per agent, on any machine you can reach with `docker context`.

It implements provider protocol v1 from upstream's
[`docs/remote-agents.md`](https://github.com/block/buzz/blob/main/docs/remote-agents.md),
and resolves the agent environment with code taken from the Kubernetes binding,
so an agent behaves the same on either substrate.

## Install

1. Put the binary for the machine that runs **Buzz Desktop** on its PATH as
   `buzz-backend-docker` (Desktop also checks `~/.local/bin`):

   ```sh
   install -m 755 buzz-backend-docker-aarch64-apple-darwin ~/.local/bin/buzz-backend-docker
   codesign -s - ~/.local/bin/buzz-backend-docker   # macOS: ad-hoc sign
   ```

   Or build it: `cargo install --git https://github.com/abipalli/buzz-backend-docker`.

2. Point a Docker context at the host that should run the agents:

   ```sh
   docker context create agents-host --docker "host=ssh://you@agents-host"
   docker --context agents-host info
   ```

   Credentials come from the context (SSH keys, TLS certs) and never from the
   provider settings.

3. In Buzz Desktop, edit an agent, choose the **docker** backend, set
   **Docker context**, and press Start.

## Settings

| Field | Default | Meaning |
|---|---|---|
| `context` | current context | `docker context` to deploy through |
| `image` | upstream `buzz-sprig`, pinned by digest | Agent runtime. Must contain `buzz-acp`; custom images should be built `FROM` buzz-sprig. `:latest` is refused |
| `network` | default bridge | Existing network to attach to. `host` is refused |
| `add_hosts` | — | `host:ip` or `host:host-gateway`, comma-separated — e.g. to reach a relay behind a reverse proxy on the Docker host |
| `ca_volume`, `ca_volume_subpath` | — | A private root CA to trust, read from a volume on the Docker host. Only that one file is mounted |
| `inactivity_seconds` | 7200 | The agent stops itself after this long without work. `0` = run until stopped |
| `cpus`, `memory` | `2`, `4g` | Container limits |

## Self-hosted relays with a private CA

Agents reach the relay over TLS. The stock `buzz-sprig` image trusts only the
public roots compiled into it, so a relay with a certificate from your own CA
fails with `invalid peer certificate: UnknownIssuer` no matter what you mount.

Until upstream reads the system store, build sprig with native roots — a
one-line change, see [`docs/native-roots.md`](docs/native-roots.md) — and set:

```json
{
  "image": "buzz-sprig-nativeroots:<commit>",
  "ca_volume": "<volume holding your CA>",
  "ca_volume_subpath": "certs/root_ca.crt"
}
```

The provider mounts the certificate at `/etc/buzz-ca/ca.crt` and sets
`SSL_CERT_DIR=/etc/ssl/certs:/etc/buzz-ca`, which adds your CA without
replacing the public roots that model APIs and git hosts need. Pointing at the
CA's own data volume (e.g. step-ca's) keeps one source of truth across
rotations.

## Lifecycle

- **Start** = deploy, and deploy converges to one live container per agent key:
  missing → create; stopped or never started → remove and recreate; running →
  no change, even if settings changed (edits apply on the next restart).
- A deploy succeeds only once the harness has stayed up for 5 seconds. If it
  exits during startup you get the exit code and the `docker logs` command,
  and the container is kept for inspection; there is exactly one create
  attempt per Start.
- **Stop** is the agent's own `!shutdown` over the relay. The container gets a
  60-second stop timeout and the harness is PID 1, so Docker's SIGTERM reaches it.
- Restart policy is `no`: an agent that stopped on purpose stays stopped. After
  a host reboot, press Start again.
- There is no undeploy in protocol v1. Deleting an agent in Desktop leaves its
  container; remove it with `docker rm`.

## Security

- The container gets the agent's private key as an environment variable, so
  anyone with access to the Docker host can read it — the same boundary as a
  Kubernetes namespace. Use a host you control.
- Values reach `docker run` through the CLI's own environment (`--env NAME`),
  never as command-line arguments.
- Containers run with `--cap-drop ALL` and `no-new-privileges`, never
  privileged or on the host network, and without host mounts other than the
  optional CA file.
- Only containers carrying this provider's label *and* the agent's full public
  key are ever removed. Anything else with the same name is reported, not touched.
- Agents run code from prompts. Give them a dedicated network rather than one
  shared with your databases.

## Develop

```sh
cargo test             # 61 tests, including the env suite inherited from upstream
cargo build --release
echo '{"op":"info"}' | target/release/buzz-backend-docker
```

## License

Apache-2.0. Portions are derived from block/buzz; see [NOTICE](NOTICE).
