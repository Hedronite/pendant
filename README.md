# Pendant

Pendant is a phone-sized web app for the **herdr** session already running on your computer.

It is not a cloud product and not a remote terminal. Herdr stays the local brain. Pendant is a thin client: a loopback HTTP bridge plus a PWA you open on your phone.

## Run locally

You need a Rust toolchain (1.85+) and, for live data, herdr 0.9.x running on the same machine.

```bash
cargo run -p pendant
```

Then open [http://127.0.0.1:8780](http://127.0.0.1:8780).

| Variable | Default |
| --- | --- |
| `PENDANT_BIND` | `127.0.0.1:8780` |
| `PENDANT_HERDR_SOCK` | `~/.config/herdr/herdr.sock` |

The process refuses `PENDANT_BIND` unless the address is loopback. `0.0.0.0`, `::`, and any other non-loopback address are refused.

## Phone access (Tailscale Serve)

On the machine running herdr and Pendant, publish the loopback bridge on your tailnet:

```bash
tailscale serve --bg --https=8443 http://127.0.0.1:8780
```

Pendant HTTPS is port `8443`, proxying `http://127.0.0.1:8780`. Port `443` is not the Pendant route. Set `PENDANT_SERVE_ORIGIN` to the exact origin a browser sends on that route. Leaving it unset makes those requests `403`.

## Security

- Tailnet-only. Pendant listens on loopback. Reach it through Tailscale Serve, not the public internet.
- No WAN bind. Do not put Pendant behind a port-forward or bind it to `0.0.0.0`.
- Anyone who can open the PWA can prompt agents, send keys, and create worktrees. Treat an unlocked phone on your tailnet as operator access.

## Workspace

| Crate | Role |
| --- | --- |
| `pendant` | Binary: runs the bridge and serves the PWA |
| `pendant-bridge` | Loopback HTTP (`/v1/…`) |
| `pendant-herdr` | herdr Unix-socket client (CLI JSON fallback) |
| `pendant-web` | Static PWA assets |

This tree is version `0.0.1` to match the crates.io name reserve. Packages are `publish = false`. Bump past `0.0.1` when a real crate ships.

## License

MIT
