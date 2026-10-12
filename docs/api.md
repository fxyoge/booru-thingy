# API coverage

Request formats were checked against Boorusama's Danbooru client at
[`e44fff7`](https://github.com/khoadng/Boorusama/tree/e44fff71cd79dc4c581f6b6072a85e999a89df37/packages/booru_clients/lib/src/danbooru).

Authenticate API requests with HTTP Basic `login:api_key` or `login` and `api_key`
query parameters. Browser sessions use an HttpOnly, SameSite cookie; mutations
must include `X-CSRF-Token`, obtained from `GET /session` or `POST /login`.
HTTPS origins use Secure cookies. API response bodies follow Danbooru shapes
rather than a new resource envelope.

ID routes accept both `/posts/1.json` and `/posts/1`. Collection routes use `.json`.
Mutations accept URL-encoded bracketed form fields or nested JSON, except raw
file uploads, which require multipart. Successful mutations return HTTP 200.

## Implemented

| Routes | Behavior |
| --- | --- |
| `GET /profile.json`, `/users.json`, `/users/:id.json` | Single owner, ID 1 |
| `GET /posts.json`, `/posts/:id.json` | Posts with tag categories and nested media variants |
| `GET /counts/posts.json` | `{"counts":{"posts":N}}`, unpaginated |
| `POST /posts.json` | Publish ready media asset; duplicates return existing post |
| `PUT /posts/:id.json` | Tags, rating, source, parent |
| `GET /tags.json` | Name list/prefix, hide-empty, count ordering, pagination |
| `GET /autocomplete.json` | Prefix tag completions |
| `GET/POST /favorites.json` | List / idempotent favorite creation |
| `DELETE /favorites/:post_id.json` | Idempotent removal by post ID |
| `GET /posts/:id/favorites.json` | Favorites with owner reference |
| `POST /uploads.json` | Multipart files or URL source (multipart/form/JSON) |
| `GET /uploads.json`, `/users/1/uploads.json`, `/uploads/:id.json` | Nested upload items, assets, and post references |
| `GET /upload_media_assets.json` | Upload items, optional `search[upload_id]` |
| `GET /media_assets.json` | Asset listing / `search[id]` selection |

`POST /uploads.json` accepts `upload[files][]` (also `upload[files]` and
`upload[file]`), `upload[source]`, `upload[referer_url]`, `upload[tag_string]`,
and `upload[rating]`. Files take precedence over URL fetching; the source remains
post metadata. A batch accepts up to 32 files. Automatic publication uses shared
tags/rating, falling back to `tagme` and `g`.

`POST /posts.json` accepts `media_asset_id` or `upload_media_asset_id` (also
under `post[...]`), plus `post[tag_string]`, `post[rating]`, `post[source]`,
and `post[parent_id]`. Existing posts are returned without changing metadata.
Use PUT for edits.

Post edits also accept query-parameter fields, including Boorusama's
`post[tag_string]` / `post[old_tag_string]` incremental editing requests.
Upload listings support `search[status]`, `search[is_posted]`, and ID ordering.

Favorites accept `post_id` in form or query input. Listing supports
`search[user_id]` and `search[post_id]`.

## Search

- Plain tags and `-excluded_tags` (AND semantics).
- `id:1,2,3`, `rating:g|s|q|e`, `md5:hash`, `source:substring`, `parent:id|none`.
- `fav:owner` / `fav:1`, `ordfav:owner` ordered newest-favorited first.
- `order:id`, `order:id_desc`, `order:id_asc`.
- `limit` from 1 to 200; numeric `page`, or post ID cursors `page=b123` / `a123`.

Unknown/unsupported metatags yield no matches. Random ordering, aliases,
implications, OR groups, numeric comparisons, and full Danbooru search parity
are not supported. Source matching treats wildcard characters literally.

## Compatibility placeholders

Empty lists are returned for AI tags, post/comment votes, comments, saved searches,
favorite groups, notes, post versions, pools, forums, messages, artists, artist URLs,
artist commentary, reports, explore results, and IQDB queries. Related tags return
a shaped empty result. Wiki and source endpoints return minimal shaped objects;
DText preview returns escaped plain text.

Mutations of unsupported features return **501** with a JSON reason rather than
claiming that data has been saved. User preference changes are currently unsupported.

## Upload-page endpoints

- `GET /uploads/new`: public page shell; collection information requires login.
- `POST /login`: JSON `{ "login": "owner", "api_key": "..." }`.
- `GET /session`, `DELETE /session`: session information / logout.
- `POST /uploads/:id/retry`: retry failed items that still have durable input.

There is no full browser post viewer or collection-management interface.
