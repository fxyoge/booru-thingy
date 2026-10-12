# Architecture

One Rust application contains the HTTP API, upload interface, and optional workers.
PostgreSQL owns all structured state. S3 owns all durable media bytes. Local files
are disposable scratch files managed by temporary directories.

## Boundaries

- `api`: Danbooru input parsing and JSON serialization, posts/tags/favorites.
- `search`: typed query terms compiled into bound PostgreSQL predicates.
- `uploads`: receive durable inputs, enqueue work, expose processing status.
- `worker`: claim/renew leases, download inputs, coordinate publication and retries.
- `media`: inspect bytes and run bounded external media tools.
- `storage`: S3 SDK adapter, private objects and signed URLs.
- `auth`: one owner credential plus hashed browser session tokens and CSRF checks.
- `web`: embedded HTML/CSS/JS, health checks, safe request logging.

No public generated database API or application-admin dashboard is exposed.

## Upload lifecycle

1. Create an upload and `receiving` items. Stream multipart file bodies to bounded
   scratch files and place them in S3 staging objects.
2. Validate shared metadata. Transactionally move all accepted items to `pending`
   and create jobs. A request is successful only after durable inputs and jobs exist.
   URL imports store the URL instead of bytes until a worker downloads it.
3. Workers claim jobs with `FOR UPDATE SKIP LOCKED`. A random lease token fences
   completion; leases last 60 seconds and are renewed every 15 seconds.
4. Fetch input, sniff format, hash bytes, validate dimensions, and extract metadata.
   Each redirect is address-checked; validated DNS answers are pinned for requests.
5. Hold a hash-scoped Postgres advisory transaction lock while checking deduplication
   and storing variants. This intentionally uses one DB connection during media
   processing to keep same-content races simple at this project's scale.
6. Publish once, update tag membership/counts, link the item, and complete its job
   in one transaction. A missing lease token rolls this transaction back.
7. Maintenance deletes staging bytes after completion. Failures retain input for
   retry; metadata records remain available in the recent-upload list.

S3 writes and database transactions cannot be atomic. Content-addressed object
keys make repeated writes harmless. A crash before commit may leave unreferenced
objects, but never a published post whose required writes have not finished.

## Media policy

Original bytes remain unchanged. `full` references the original. Thumbnail variants
fit within 180, 360, and 720 pixel bounds without stretching or upscaling. Still
image samples fit within 1600 pixels. Animated/video samples reference originals
so animation is preserved; thumbnails are static first-frame JPEGs.

Each derivative stores actual dimensions and size. Objects use
`media/{sha256}/original.{ext}` and `media/{sha256}/v1/{kind}.jpg`.

Direct signed URLs avoid forcing large downloads through the backend. Cached API
responses contain expiring URLs; refreshing the post obtains new URLs. Verify
Boorusama's cache/download behavior when qualifying a release.

## Consistency

Post edits lock the post and apply normalized tag differences transactionally.
`old_tag_string` supports three-way edits and incremental removal tokens, preserving
unrelated concurrent tag additions. Tag counters update only for changed memberships.
Favorites have a unique post constraint; counts are derived, avoiding counter drift.
Search counts use the same predicates as listings without pagination.
