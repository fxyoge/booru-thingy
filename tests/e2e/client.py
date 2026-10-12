"""External client helpers. No application imports or deployment knowledge."""

import hashlib
import io
import time

import httpx
from PIL import Image


class Client:
    def __init__(self, base_url, login, key):
        self.http = httpx.Client(
            base_url=base_url, auth=(login, key), timeout=20, trust_env=False
        )

    def request(self, method, path, expected=200, **kwargs):
        response = self.http.request(method, path, **kwargs)
        assert response.status_code == expected, (
            f"{method} {path}: expected {expected}, got {response.status_code}\n"
            f"{response.text[:8000]}"
        )
        return response

    def get(self, path, **params):
        return self.request("GET", path, params=params).json()

    def upload(self, files, tags="", rating="g", source="", expected=200):
        upload = self.reserve(files, tags, rating, source, expected)
        if expected != 200:
            return upload
        for item, (name, content) in zip(upload["upload_media_assets"], files):
            ticket = self.ticket(upload["id"], item["id"])
            response = send_ticket(ticket, name, content)
            assert response.status_code == 204, response.text
            self.complete(upload["id"], item["id"])
        return upload

    def reserve(self, files, tags="", rating="g", source="", expected=200):
        return self.request("POST", "/uploads/direct", expected=expected, json={
            "files": [{"filename": name, "size": len(content)} for name, content in files],
            "tags": tags, "rating": rating, "source": source,
        }).json()

    def ticket(self, upload_id, item_id, expected=200):
        return self.request("POST", f"/uploads/{upload_id}/items/{item_id}/ticket", expected=expected).json()

    def complete(self, upload_id, item_id, expected=200):
        return self.request("POST", f"/uploads/{upload_id}/items/{item_id}/complete", expected=expected).json()

    def import_url(self, url, tags="", expected=200):
        return self.request(
            "POST", "/uploads.json", expected=expected,
            json={"upload": {"source": url, "tag_string": tags}},
        ).json()

    def await_upload(self, upload_id, status="completed", timeout=120):
        deadline = time.monotonic() + timeout
        last = None
        while time.monotonic() < deadline:
            last = self.get(f"/uploads/{upload_id}.json")
            if last["status"] in ("completed", "error"):
                assert last["status"] == status, f"Upload {upload_id}: {last}"
                return last
            time.sleep(0.25)
        raise AssertionError(f"Upload {upload_id} timed out; last response: {last}")

    def publish(self, fixture, tags="", **kwargs):
        accepted = self.upload([(fixture.name, fixture.read_bytes())], tags, **kwargs)
        result = self.await_upload(accepted["id"])
        assert len(result["upload_media_assets"]) == 1, result
        item = result["upload_media_assets"][0]
        assert item["status"] == "completed" and item["error"] is None, item
        return self.get(f"/posts/{item['post_id']}.json")

    def posts(self, tags, **params):
        return self.get("/posts.json", tags=tags, **params)

    def count(self, tags, **params):
        return self.get("/counts/posts.json", tags=tags, **params)["counts"]["posts"]

    def edit(self, post_id, expected=200, **fields):
        return self.request(
            "PUT", f"/posts/{post_id}.json", expected=expected, json={"post": fields}
        ).json()


def send_ticket(ticket, name, content, fields=None, headers=None):
    # Never forward the application's Basic authentication or session to storage.
    with httpx.Client(timeout=30, trust_env=False) as http:
        return http.post(ticket["url"], data=fields if fields is not None else ticket["fields"],
                         files={"file": (name, content, "application/octet-stream")}, headers=headers)


def download(url):
    # Media requests intentionally carry no API credentials or session cookies.
    with httpx.Client(timeout=20, trust_env=False) as http:
        response = http.get(url)
    assert response.status_code == 200, f"Media download: {response.status_code} {response.text[:500] if response.is_error else ''}"
    return response


def assert_original(post, fixture):
    expected = fixture.read_bytes()
    response = download(post["file_url"])
    assert response.content == expected, f"Post {post['id']}: original bytes differ"
    assert post["file_size"] == len(expected)
    assert post["md5"] == hashlib.md5(expected).hexdigest()


def assert_jpeg_variant(variant):
    response = download(variant["url"])
    assert response.headers["content-type"].split(";")[0] == "image/jpeg"
    assert len(response.content) == variant["file_size"]
    with Image.open(io.BytesIO(response.content)) as image:
        image.load()
        assert image.format == "JPEG"
        assert image.size == (variant["width"], variant["height"])
    return image.size
