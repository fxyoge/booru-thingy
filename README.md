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
- A single Docker image; no persistent application filesystem or separate queue.

This is a deliberately small backend, not a full Danbooru implementation. See
[API coverage](docs/api.md) for supported behavior. Compatibility targets
Boorusama's Danbooru client.

## Quick start

Install Docker with Compose. Copy `.env.example` to `.env`, set a long random
`OWNER_API_KEY`, then run:

```sh
docker compose up --build -d
```

Open **http://localhost:8080/uploads/new** and sign in with `OWNER_LOGIN` and
`OWNER_API_KEY`. Uploads publish automatically once processing finishes. Empty
tags become `tagme`; the default rating is General.

Compose includes PostgreSQL and MinIO for local use. Its database and object
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

## Bring your own services

Build the image:

```sh
docker build -t booru-thingy .
docker run --rm --env-file .env -p 8080:8080 booru-thingy
```

The database and bucket must already exist. The default command applies embedded
database migrations, then runs the HTTP server and upload workers. Put your
preferred HTTPS reverse proxy in front and set `BASE_URL` to its external origin.

Required configuration:

| Variable | Purpose |
| --- | --- |
| `DATABASE_URL` | PostgreSQL connection URL |
| `OWNER_API_KEY` | Owner's API credential |
| `S3_ENDPOINT` | S3-compatible service endpoint |
| `S3_BUCKET` | Existing bucket |
| `AWS_ACCESS_KEY_ID` | Bucket access key |
| `AWS_SECRET_ACCESS_KEY` | Bucket secret key |

Additional configuration:

| Variable | Default | Purpose |
| --- | --- | --- |
| `BASE_URL` | `http://localhost:8080` | External origin; browser origin validation and secure cookies |
| `OWNER_LOGIN` | `owner` | Single owner login |
| `LISTEN_ADDR` | `0.0.0.0:8080` | HTTP bind address |
| `S3_REGION` | `us-east-1` | S3 signing region |
| `S3_PATH_STYLE` | `true` | Path-style rather than virtual-host addressing |
| `S3_PUBLIC_ENDPOINT` | same as `S3_ENDPOINT` | Client-reachable endpoint for signed media URLs |
| `MEDIA_URL_TTL_SECONDS` | `3600` | Signed URL lifetime; maximum seven days |
| `MAX_UPLOAD_BYTES` | `536870912` | Maximum total file bytes per batch / downloaded URL |
| `WORKER_CONCURRENCY` | `2` | Concurrent processing jobs per worker process |
| `PROCESS_TIMEOUT_SECONDS` | `300` | Overall processing deadline per item |
| `ALLOW_PRIVATE_SOURCE_URLS` | `false` | Allow URL imports from private/reserved network addresses |
| `RUST_LOG` | `booru_thingy=info` | Log filter |

Bucket access needs object read/write/delete and bucket HEAD permission. Media
objects stay private; clients receive expiring signed GET URLs. The public S3
endpoint must expose the same bucket with the same addressing mode and region.

### Process modes

```sh
booru-thingy          # apply schema, serve API, run workers
booru-thingy migrate  # apply schema and exit
booru-thingy serve    # API only, schema must already be current
booru-thingy worker   # workers only, schema must already be current
```
