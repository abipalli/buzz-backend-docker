# buzz-backend-docker

**Run your [Buzz](https://github.com/block/buzz) agents on your own server.**
Close the laptop; they keep working.

Buzz Desktop runs agents on your machine by default. It can also hand an agent
to a *backend provider*, which starts it somewhere else. This is a provider for
any Docker host: a home server, a VPS, a spare Mac mini. Each agent becomes one
container, and from then on it lives on the relay like any other member —
you see it online, you mention it to work with it, and `!shutdown` stops it.

## Quick start

**Recommended:** follow the [setup guide](https://github.com/abipalli/buzz-backend-docker/wiki/Recommended-setup).
It connects your Mac and server over [Tailscale](https://tailscale.com) and
gives the server a short SSH profile name, so nothing is exposed to the
internet. With that in place (`ssh agents` works and the server user can run
`docker`), on the Mac that runs Buzz Desktop:

```sh
brew install abipalli/tap/buzz-backend-docker
buzz-backend-docker setup ssh://agents
```

`setup` makes the provider visible to Buzz Desktop and checks that Docker on
the server answers. Then in Buzz Desktop, open an agent's settings, choose the
**docker** backend, set **Docker host** to `ssh://agents`, and press **Start**.

Any `ssh://user@host[:port]` works too; the profile just keeps keys, ports and
connection reuse in `~/.ssh/config`. The wiki also covers
[exposing the relay](https://github.com/abipalli/buzz-backend-docker/wiki/Exposing-the-relay)
with Tailscale Serve or Traefik, and
[troubleshooting](https://github.com/abipalli/buzz-backend-docker/wiki/Troubleshooting).

## Settings

Only **Docker host** is needed; the rest have working defaults.

| Setting | Default | What it does |
|---|---|---|
| Docker host | this machine | The server that runs the agent, e.g. `ssh://agents` (an SSH profile) or `ssh://you@your-server`. Uses your SSH setup; never put a password here |
| Docker context | — | Alternative to Docker host: a name from `docker context ls`. Ignored when Docker host is set |
| Agent image | `buzz-sprig` with native TLS roots, pinned | The agent runtime. Custom images must be built `FROM` it. `:latest` is refused |
| Docker network | default bridge | Network to join; created if missing. Give agents their own; `host` is refused |
| Extra hosts | — | `name:ip` or `name:host-gateway`, for names the server's DNS can't resolve |
| CA volume / CA file in volume | — | A private root CA for a self-hosted relay (below). Only that one file is mounted |
| Stop after inactivity | 7200 s | The agent stops itself after this long without work. `0` keeps it running |
| CPU / memory limit | `2` / `4g` | Container limits |

Secrets such as model API keys go in the agent's **environment variables** in
Desktop, not in these settings.

## Self-hosted relay with a private CA

If your relay's certificate comes from your own CA, point **CA volume** at the
Docker volume on the server that holds the root certificate and set **CA file
in volume** to its path — for step-ca, its data volume and `certs/root_ca.crt`.
The provider adds that CA next to the public ones, so model APIs and git hosts
keep working, and a rotated root is picked up on the next Start.

This works because the default agent image is upstream's `buzz-sprig` built
with native TLS roots; the stock image only trusts roots compiled into it. The
image is rebuilt from upstream by this repo's `sprig-image` workflow; see
[docs/native-roots.md](docs/native-roots.md).

## How it behaves

- **Start converges.** No container → create one. Stopped or never started →
  replace it. Running → leave it alone; setting changes apply on its next start.
- **Start succeeds only if the agent stays up.** If it exits during startup,
  you get the exit code and the `docker logs` command, and the container is
  kept for you to inspect. There is one attempt per Start.
- **Stop is the agent's own `!shutdown`.** Docker's stop signal reaches the
  agent directly with a 60-second grace period.
- **Stopped stays stopped.** The restart policy is `no`; after a server reboot,
  press Start again.
- **Deleting an agent in Desktop leaves its container.** Remove it with
  `docker -H ssh://you@your-server rm buzz-agent-<id>`.

## Security

- The agent's private key is in the container's environment. Anyone who can
  run `docker` on that host can read it — use a host you control.
- Values reach Docker through the CLI's environment, never its arguments.
- Containers drop all Linux capabilities and can't gain privileges. The
  provider never uses privileged mode, the host network, or host paths.
- The provider only removes containers it created for that exact agent.
  Anything else with the same name is reported and left alone.

## How it works

Buzz Desktop finds any executable named `buzz-backend-<id>` and talks to it
with one JSON request on stdin and one response on stdout:
`info` describes the settings form, `deploy` starts the agent. This provider
implements protocol v1 from upstream's
[`docs/remote-agents.md`](https://github.com/block/buzz/blob/main/docs/remote-agents.md)
and shares its environment-building code with upstream's Kubernetes provider,
so an agent behaves the same on either.

```sh
echo '{"op":"info"}' | buzz-backend-docker
```

Agent containers are named `buzz-agent-<first 12 hex of the agent's pubkey>`
and labeled `app.kubernetes.io/managed-by=buzz-backend-docker`.

## Development

```sh
cargo test
cargo clippy --all-targets -- -D warnings
```

Releases: bump `version` in `Cargo.toml`, then push a matching tag
(`git tag v0.2.0 && git push --tags`). CI builds macOS and Linux binaries,
publishes the GitHub release, updates the
[Homebrew tap](https://github.com/abipalli/homebrew-tap), and installs it on a
macOS runner to check.

## License

Apache-2.0. Parts are derived from block/buzz; see [NOTICE](NOTICE).
