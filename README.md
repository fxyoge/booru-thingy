# booru-thingy

A small, single-user, headless booru backend built for **Boorusama**.

Keep a personal collection of images and videos, browse and edit it from your
booru client, and add new finds through a focused, mobile-friendly upload page.
Rust powers the application, PostgreSQL holds metadata, and S3-compatible object
storage holds originals and previews.

## Features

- Danbooru-shaped post, tag, profile, favorite, and upload APIs.
- Tag search, exclusions, common metatags, autocomplete, and accurate counts.
- Tag/rating/source edits, parent relationships, and favorites.
- Drag-and-drop, clipboard paste, batch uploads, and direct media URL imports.
- Automatic publication with configurable tags and rating on the upload page.
- Restart-safe background processing, bounded retries, and content deduplication.
- JPEG, PNG, GIF, WebP, MP4, and WebM originals with generated static previews.

## Quick start

Install Docker with Compose. Copy `.env.example` to `.env`, set a long random
`OWNER_API_KEY`, and replace the database and storage credentials, then run:

```sh
docker compose up --build -d
```

Open **http://localhost:8080/uploads/new** and sign in with `OWNER_LOGIN` and
`OWNER_API_KEY`. Uploads publish automatically once processing finishes. Empty
tags become `tagme`; the default rating is General.

Compose includes PostgreSQL and SeaweedFS for local use. Its database and object
store volumes are owned by those services; the application itself is stateless.
The example S3 public endpoint is `localhost`, so remote clients need it changed
to an address they can reach.

### Boorusama

Add a custom **Danbooru** site using the backend's external URL. Enter the
configured owner login as the username and the owner API key as the API key.
The owner has user ID `1`.

Android URL sharing opens `/uploads/new?url=...`; sign in in the browser to
complete the upload. URL imports accept direct image/video links rather than
scraping media from websites.

## Usage

booru-thingy merges configs from these locations in order of highest priority
to least:

* config override passed a CLI arg e.g. `--listen-addr 0.0.0.0:8080`
* env vars such as `S3_ENDPOINT`, `S3_BUCKET`...
* config file passed as a CLI arg `--config path/to/config.toml` (or `BOORU_CONFIG`)
* current working directory `./config.toml`
* XDG config location `$XDG_CONFIG_HOME/booru-thingy/config.toml` (or `$HOME/.config/booru-thingy/config.toml`)
* built-in defaults

See [`config.example.toml`](config.example.toml) for example configuration
values and documentation.

<!-- BEGIN GENERATED CLI HELP -->

```text
A small headless booru backend

Usage: booru-thingy [OPTIONS] <COMMAND>

Commands:
  run      Apply migrations, then run the API and upload workers in one process
  serve    Serve the API only
  worker   Run upload workers only
  migrate  Apply database migrations and exit
  help     Print this message or the help of the given subcommand(s)

Options:
      --config <CONFIG>
          Merge this TOML file over the discovered files [env: BOORU_CONFIG]
      --database-url <DATABASE_URL>

      --listen-addr <LISTEN_ADDR>
          HTTP bind address
      --base-url <BASE_URL>
          External HTTP(S) origin, without a path
      --owner-login <OWNER_LOGIN>

      --owner-api-key <OWNER_API_KEY>

      --s3-endpoint <S3_ENDPOINT>

      --s3-public-endpoint <S3_PUBLIC_ENDPOINT>

      --s3-region <S3_REGION>

      --s3-bucket <S3_BUCKET>

      --aws-access-key-id <AWS_ACCESS_KEY_ID>

      --aws-secret-access-key <AWS_SECRET_ACCESS_KEY>

      --s3-path-style <S3_PATH_STYLE>
          [possible values: true, false]
      --media-url-ttl-seconds <MEDIA_URL_TTL_SECONDS>

      --upload-ticket-ttl-seconds <UPLOAD_TICKET_TTL_SECONDS>
          Signed browser upload ticket lifetime, in seconds
      --max-upload-bytes <MAX_UPLOAD_BYTES>
          Maximum total file bytes per upload batch or downloaded URL
      --worker-concurrency <WORKER_CONCURRENCY>
          Concurrent workflow steps per worker process
      --process-timeout-seconds <PROCESS_TIMEOUT_SECONDS>
          Deadline per workflow step attempt, in seconds
      --allow-private-source-urls <ALLOW_PRIVATE_SOURCE_URLS>
          [possible values: true, false]
      --rust-log <RUST_LOG>
          Tracing log filter
  -h, --help
          Print help
  -V, --version
          Print version
```

<!-- END GENERATED CLI HELP -->
