![cover image](./static/og-image.png)

**B{rew}log** is a self-hosted specialty coffee logging platform optimised for filter brewing
enthusiasts. B{rew}log can be used for tracking roasters, roasts, brews, cafes and brewing gear.

B{rew}log features an LLM-powered "Bag Scanning" feature, which enables it to automatically fill
roaster and coffee information using a photo of a bag. It also supports "check-ins" to log coffee
enjoyed in a cafe.

Create and edit forms keep their save actions disabled until required fields and entered values
are valid. Invalid submissions sent directly to the API still receive validation errors.

B{rew}log ships as a single Rust binary that serves a web UI, a REST API, and a CLI client. The
application uses SQLite as a backend, and will automatically create and migrate the database on
start-up.

> [!NOTE]
> This application was built almost entirely with Claude Code. I used this project as an
> excuse to explore the current state of the art in agentic coding tools. I've reviewed
> much of the code, and I'm largely responsible for the rules and the layout of the repository
> but nonetheless the vast majority of the code was written by Claude and Opus 4.6.

## Quick Start (Demo)

Before you start, you'll need to sign up for [Openrouter](https://openrouter.ai) and
[Foursquare Places](https://foursquare.com/products/places/) and get API keys for both.

Then create a `docker.env` file:

```env
# You'll need an API key from OpenRouter
BREWLOG_OPENROUTER_API_KEY=sk-or-...
# I've had good results with Gemini models, but you can try 'openrouter/free' to experiment
BREWLOG_OPENROUTER_MODEL=google/gemini-3-flash-preview
# FourSquare Places API key for location searching
BREWLOG_FOURSQUARE_API_KEY=fsq3...
```

You can see the full list of configuration options [below](#configuration). Once your `.env` file
is complete, start the container using the environment file

```bash
# Create a data directory to store the database
mkdir data
# Run the container
docker run \
  --rm \
  -p 3000 \
  --env-file docker.env \
  -v $PWD/data:/data \
  ghcr.io/jnsgruk/brewlog:latest
```

On first start with an empty database the server prints a one-time registration URL:

```
No users found. Register the first user at:
  http://localhost:3000/register/abc123...
This link expires in 1 hour.
```

Open that URL, choose a display name, and register a passkey. This creates an account and
signs in automatically.

### Native OpenID Connect

The MaxRink fork adds optional native OpenID Connect. Set
`BREWLOG_OIDC_ISSUER`, `BREWLOG_OIDC_CLIENT_ID`, `BREWLOG_OIDC_REDIRECT_URL`
(normally `https://brewlog.example/auth/oidc/callback`) and, when required by
the provider, `BREWLOG_OIDC_CLIENT_SECRET`. The flow uses provider discovery,
authorization code + PKCE, state and nonce validation, and signed ID-token
validation. Identities are linked by the immutable `(issuer, subject)` pair;
email is only a username hint and cannot take over an existing local account.
Passkeys remain available as local recovery. Leave these variables unset to
retain upstream-only passkey behavior.

This fork is maintained at `https://github.com/MaxRink/brewlog`, based on
`https://github.com/jnsgruk/brewlog`. The homelab pins the deployed revision by
container digest. Rebase this small OIDC patch onto upstream releases, run the
full test suite, and review the migration before changing that digest.

For the homelab Authelia client, register a confidential web client with issuer
`https://auth.private-testumgebung.de` and the callback
`https://brewlog.private-testumgebung.de/auth/oidc/callback`. Request only the
`openid profile email` scopes. The authorization-code flow uses S256 PKCE and
the client secret stays in the Arcane secret store. A user can sign in through
OIDC to provision a normal Brewlog account, or sign in locally first and use
the Admin page's explicit **Link Authelia account** action. Linking is
session-bound and uses only the verified `(issuer, sub)` identity; email and
name claims never select, merge, or take over an account. Brewlog has no OIDC
role mapping, so no OIDC claim grants admin access. Passkeys remain the local
recovery method.

To require OIDC for new accounts, set `BREWLOG_OIDC_ONLY=true`. Brewlog refuses
to start in this mode unless the issuer, client ID, and redirect URL above are
all configured. OIDC-only mode disables the native registration page and API;
passkey sign-in and passkey management remain available for existing accounts
as a local recovery path. This setting does not grant an OIDC user admin access.

The reviewed fork revision is `f155555d85314412db0496eaebfc8b89d13cc6d4`.
The corresponding image artifact must be pinned by its immutable image digest
when imported into Arcane; do not deploy a mutable branch or tag.

### Install from Git

To build and install from source, you'll need a working Rust toolchain:

```bash
cargo install --locked --git https://github.com/jnsgruk/brewlog.git
```

Then create a `.env` file containing at least your OpenRouter and Foursquare API keys, and start
the server:

```bash
brewlog serve
```

### CLI Authentication

To use the CLI or API for write operations, create a token via browser hand-off:

```bash
brewlog token create --name "my-cli-token"
# Browser opens → authenticate with a passkey → token printed once

export BREWLOG_URL="http://localhost:3000"
export BREWLOG_TOKEN="<token from above>"

# Create data from the CLI
brewlog roaster add --name "Radical Roasters" --country "United Kingdom"
```

Run `brewlog --help` for the full command reference.

## Configuration

All settings are read from environment variables or CLI flags. A `.env` file in the working
directory is loaded automatically via [dotenvy](https://crates.io/crates/dotenvy).

### Server (`brewlog serve`)

| Variable                   | Purpose                                                                | Default                 |
| -------------------------- | ---------------------------------------------------------------------- | ----------------------- |
| `BREWLOG_RP_ID`            | WebAuthn Relying Party ID (server domain)                              | `localhost`             |
| `BREWLOG_RP_ORIGIN`        | WebAuthn Relying Party origin (full URL)                               | `http://localhost:3000` |
| `BREWLOG_DATABASE_URL`     | Database connection string                                             | `sqlite://brewlog.db`   |
| `BREWLOG_BIND_ADDRESS`     | Server bind address                                                    | `127.0.0.1:3000`        |
| `BREWLOG_INSECURE_COOKIES` | Disable the `Secure` cookie flag (auto-enabled for localhost defaults) | `false`                 |
| `BREWLOG_OIDC_ISSUER` | OIDC issuer URL; enables native OIDC when all required OIDC settings are present | — |
| `BREWLOG_OIDC_CLIENT_ID` | OIDC client identifier | — |
| `BREWLOG_OIDC_CLIENT_SECRET` | OIDC client secret, if the provider requires one | — |
| `BREWLOG_OIDC_REDIRECT_URL` | Registered callback URL (`/auth/oidc/callback`) | — |
| `BREWLOG_OIDC_ONLY` | Disable native account registration; requires complete OIDC configuration | `false` |
| `RUST_LOG`                 | Log level filter                                                       | `info`                  |
| `RUST_LOG_FORMAT`          | Set to `json` for structured log output                                | —                       |
| `BREWLOG_METRICS_ENABLED`  | Set to `true` to expose bounded Prometheus metrics at `/metrics`       | `false`                 |
| `BREWLOG_OTEL_ENABLED`     | Set to `true` with the `otlp` build feature to export bounded spans    | `false`                 |
| `OTEL_EXPORTER_OTLP_ENDPOINT` | OTLP gRPC collector endpoint when OTLP is enabled                  | —                       |

When metrics are enabled, request counters and duration histograms use only
route templates, HTTP methods and status classes. Request bodies, query
strings, cookies, images, tasting notes and user identifiers are excluded.
The optional `otlp` build feature adds OTLP gRPC spans for the same bounded
request metadata; the exporter is disabled unless both
`BREWLOG_OTEL_ENABLED` and `OTEL_EXPORTER_OTLP_ENDPOINT` are set.

### CLI Client

| Variable        | Purpose                               | Default                 |
| --------------- | ------------------------------------- | ----------------------- |
| `BREWLOG_URL`   | Server URL                            | `http://localhost:3000` |
| `BREWLOG_TOKEN` | API bearer token for write operations | —                       |

### Integrations

| Variable                     | Purpose                                                                     | Default       |
| ----------------------------- | ---------------------------------------------------------------------------- | ------------- |
| `BREWLOG_INFERENCE_PROVIDER`  | AI inference backend: `openrouter` or `openai-compatible`                    | `openrouter`  |
| `BREWLOG_FOURSQUARE_API_KEY`  | [Foursquare](https://foursquare.com/) Places API key for optional nearby cafe search  | optional  |

#### `openrouter` provider (default)

A preset for [OpenRouter](https://openrouter.ai/)'s own API — same endpoint, same env vars as
before, plus the `openrouter:web_search` server tool on every extraction request.

| Variable                     | Purpose                          | Default            |
| ----------------------------- | --------------------------------- | ------------------ |
| `BREWLOG_OPENROUTER_API_KEY` | OpenRouter API key                | **required**       |
| `BREWLOG_OPENROUTER_MODEL`   | LLM model for AI extraction       | `openrouter/free`  |

#### `openai-compatible` provider

Routes AI extraction through any OpenAI Chat Completions-compatible endpoint (e.g. an in-cluster
LiteLLM proxy) instead of OpenRouter. Sends no server-side tools.

| Variable                      | Purpose                                                                              | Default                                    |
| ------------------------------ | -------------------------------------------------------------------------------------- | ------------------------------------------- |
| `BREWLOG_INFERENCE_BASE_URL`  | Chat-completions endpoint                                                            | **required**                                |
| `BREWLOG_INFERENCE_API_KEY`   | API key, sent as a bearer token. Omitted when unset (e.g. the proxy injects its own)  | —                                            |
| `BREWLOG_INFERENCE_MODEL`     | LLM model. Omitted from the request when unset, so the proxy picks its own default   | —                                            |

## Development Setup

Install [mise](https://mise.jdx.dev/) then set up the development environment:

```bash
sudo apt update
sudo apt install -y clang mold pkg-config libssl-dev \
    libatk1.0-0 libatk-bridge2.0-0 libcups2 libdrm2 libxkbcommon0 \
    libxcomposite1 libxdamage1 libxrandr2 libgbm1 libpango-1.0-0 \
    libcairo2 libasound2t64 libnss3 libxshmfence1
mise trust
mise install              # Install all dev tools
mise run install-e2e      # Install Chrome for Testing + ChromeDriver
prek install              # Install git hooks
```

## Contributing

```bash
prek run -av              # Run all lints, tests, formatters
mise run fmt              # Format all files
mise run check            # Full CI validation (fmt + lint + test)
mise run test             # Run all tests
cargo build               # Build
```

See [CLAUDE.md](CLAUDE.md) for architecture, code patterns, and development conventions.

## License

[Apache License 2.0](LICENSE)
