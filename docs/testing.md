# Testing

`cargo test` runs isolated parser, tag-edit, format-sniffing, and source-address
tests. No infrastructure is needed for those tests.

The ignored integration test creates a disposable database using SQLx and a
unique S3 bucket. Its PostgreSQL role needs CREATEDB permission. Use a dedicated
test object store; test buckets and objects are intentionally left for inspection.

```sh
export DATABASE_URL=postgres://test:test@localhost:5432/test
export TEST_S3_ENDPOINT=http://localhost:9000
export TEST_S3_ACCESS_KEY=local-access-key
export TEST_S3_SECRET_KEY=local-secret-key
cargo test -- --ignored
```

Install ffmpeg and libvips tools first. The integration test exercises:

- Browser sessions, Basic/query auth, CSRF, and origin validation.
- Durable multipart inputs and reclaiming an expired worker lease.
- All six supported formats and actual S3-backed previews.
- Concurrent same-content deduplication and metadata preservation.
- Post query-parameter edits matching Boorusama's request format.
- Accurate unpaginated counts, exclusions, tag counters, and favorites.
- Terminal processing errors for invalid media.
- Animated WebP, URL redirects, and recovery after cancelling a live worker.

The optional browser smoke test exercises the actual upload interface against a
running disposable instance (it creates posts):

```sh
pip install playwright==1.51.0
playwright install chromium --with-deps
OWNER_API_KEY=your-test-key python tests/browser_smoke.py
```

Set `TEST_BASE_URL` and `OWNER_LOGIN` if needed. It checks login/logout,
file previews, publication, autocomplete, shared-URL handling, dark mode, and
mobile overflow. CI runs this alongside the Rust integration test and Docker build.

## Device release check

Configure a custom Danbooru site in Boorusama and exercise:

1. Sign in, open profile, browse pages, and search positive/negative tags.
2. View stills, animated GIF/WebP, MP4, and WebM; seek and download video.
3. Favorite/unfavorite and browse ordered favorites.
4. Add/remove tags and change rating; confirm edits after refreshing.
5. Share a direct media URL to Boorusama, sign in in the browser, and upload.
6. Batch-upload files through the page; verify recent statuses and client uploads.
7. Refresh a post after its signed media URL expires; verify client cache behavior.
8. Open placeholder screens and confirm empty states are handled gracefully.

Automated contract checks do not replace a real-device client smoke test.
