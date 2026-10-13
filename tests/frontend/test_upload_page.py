"""Browser/network regressions against the real frontend, with a deterministic API.

Run: python -m pytest tests/frontend (requires pytest-playwright and Chromium).
No database, credentials, storage server, or running app required.
"""

import base64
import copy
import json
from pathlib import Path
from urllib.parse import parse_qs, urlparse

import pytest
from playwright.sync_api import expect


ROOT = Path(__file__).resolve().parents[2]
PNG = base64.b64decode("iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mP8/x8AAwMCAO+aWZkAAAAASUVORK5CYII=")


def upload(identity=1, processing=False):
    item = {"id": identity * 10, "filename": f"image-{identity}.png", "status": "completed",
            "post_id": identity, "duplicate": False, "error": None,
            "media_asset": {"id": identity, "variants": [{"type": "180x180", "url": f"http://upload.test/thumb/{identity}?sig=initial"}]}}
    return {"id": identity, "created_at": "2026-10-13T00:00:00Z", "status": "processing" if processing else "completed",
            "upload_media_assets": [item] + ([{"id": identity * 10 + 1, "filename": "pending.png", "status": "pending"}] if processing else [])}


@pytest.fixture
def site(page):
    class Site:
        uploads = [upload()]
        requests = []
        errors = []
        polls = 0
        complete_after = 2
        retry = False
        fail_polls = False
        authenticated = True
        fail_storage_once = False

        def route(self, route):
            request = route.request
            url = urlparse(request.url)
            path = url.path
            self.requests.append((request.method, path))
            if path == "/uploads/new":
                route.fulfill(content_type="text/html", body=(ROOT / "templates/upload.html").read_text().replace("{{MAX_BYTES}}", "2097152"))
                return
            if path.startswith("/assets/"):
                route.fulfill(content_type="text/css" if path.endswith(".css") else "text/javascript",
                              body=(ROOT / path.lstrip("/")).read_text())
                return
            if path.startswith("/thumb/"):
                route.fulfill(content_type="image/png", body=PNG)
                return
            if path == "/session":
                if request.method == "DELETE":
                    self.authenticated = False
                    data = {}
                elif not self.authenticated:
                    route.fulfill(status=401, json={"reason": "Sign in"})
                    return
                else:
                    data = {"name": "owner", "csrf_token": "csrf"}
            elif path == "/login":
                assert request.post_data_json == {"login": "owner", "api_key": "key"}
                self.authenticated = True
                data = {"name": "owner", "csrf_token": "csrf"}
            elif path == "/autocomplete.json":
                data = [{"value": "test_tag", "post_count": 3}]
            elif path == "/uploads.json":
                data = upload(2)
                self.uploads.insert(0, copy.deepcopy(data))
            elif path == "/users/1/uploads.json":
                limit = int(parse_qs(url.query)["limit"][0])
                data = copy.deepcopy(self.uploads[:limit])
            elif path.endswith("/retry"):
                self.retry = True
                self.uploads[0]["status"] = "processing"
                self.uploads[0]["upload_media_assets"][0]["status"] = "pending"
                self.uploads[0]["upload_media_assets"][0]["error"] = None
                data = self.uploads[0]
            elif path == "/uploads/1":
                self.polls += 1
                if self.fail_polls:
                    route.fulfill(status=503, json={"reason": "Temporarily unavailable"})
                    return
                index = next(i for i, upload in enumerate(self.uploads) if upload["id"] == 1)
                data = copy.deepcopy(self.uploads[index])
                for item in data["upload_media_assets"]:
                    if "media_asset" in item:
                        item["media_asset"]["variants"][0]["url"] = f"http://upload.test/thumb/1?sig=poll-{self.polls}"
                if self.polls >= self.complete_after:
                    data["status"] = "completed"
                    for item in data["upload_media_assets"]:
                        item.update(status="completed", post_id=1)
                    self.uploads[index] = copy.deepcopy(data)
            elif path == "/uploads/direct":
                files = request.post_data_json["files"]
                data = {"id": 2, "created_at": "2026-10-13T00:00:00Z", "status": "processing",
                        "upload_media_assets": [{"id": 20 + i, "filename": file["filename"], "status": "receiving"} for i, file in enumerate(files)]}
                self.uploads.insert(0, copy.deepcopy(data))
            elif path.endswith("/ticket"):
                data = {"url": "http://upload.test/storage", "fields": {}}
            elif path == "/storage":
                if self.fail_storage_once:
                    self.fail_storage_once = False
                    route.fulfill(status=503)
                    return
                route.fulfill(status=204, headers={"ETag": '"test"'})
                return
            elif path.endswith("/complete"):
                identity = int(path.split("/")[-2])
                data = self.uploads[0]
                for item in data["upload_media_assets"]:
                    if item["id"] == identity:
                        item.update(status="completed", post_id=identity)
                if all(item["status"] == "completed" for item in data["upload_media_assets"]):
                    data["status"] = "completed"
            else:
                raise AssertionError(f"Unexpected request: {request.method} {request.url}")
            route.fulfill(content_type="application/json", body=json.dumps(data))

        def open(self):
            page.goto("http://upload.test/uploads/new")
            expect(page.get_by_test_id("workspace")).to_be_visible()
            expect(page.get_by_test_id("upload-card")).to_have_count(min(10, len(self.uploads)))

        def count(self, path):
            return sum(p == path for _, p in self.requests)

    server = Site()
    page.route("http://upload.test/**", server.route)
    page.on("pageerror", lambda error: server.errors.append(str(error)))
    yield server
    assert not server.errors


def test_idle_queue_does_not_poll_or_reload_images(page, site):
    site.open()
    expect(page.get_by_test_id("upload-thumbnail")).to_be_visible()
    page.wait_for_function("document.querySelector('[data-testid=upload-thumbnail]').naturalWidth > 0")
    page.wait_for_timeout(4500)
    assert site.count("/users/1/uploads.json") == 1
    assert site.count("/uploads/1") == 0
    assert site.count("/thumb/1") == 1


def test_active_updates_preserve_thumbnail_and_stop_when_completed(page, site):
    site.uploads = [upload(processing=True)]
    site.open()
    page.get_by_test_id("upload-thumbnail").first.evaluate("el => window.originalThumb = el")
    expect(page.get_by_test_id("queue-status")).to_have_text("Processing in the background")
    expect(page.get_by_test_id("queue-status")).to_be_empty(timeout=10000)
    expect(page.get_by_test_id("upload-item").last).to_have_attribute("data-status", "completed")
    assert page.evaluate("originalThumb === document.querySelector('[data-testid=upload-thumbnail]')")
    assert site.count("/users/1/uploads.json") == 1
    assert site.count("/thumb/1") == 1
    assert site.polls == 2
    page.wait_for_timeout(2500)
    assert site.polls == 2


def test_pagination_and_empty_reconciliation_preserve_existing_nodes(page, site):
    site.uploads = [upload(i) for i in range(11, 0, -1)]
    site.open()
    page.get_by_test_id("upload-thumbnail").first.evaluate("el => window.originalThumb = el")
    page.get_by_test_id("more-uploads").click()
    expect(page.get_by_test_id("upload-card")).to_have_count(11)
    assert page.evaluate("originalThumb === document.querySelector('[data-testid=upload-thumbnail]')")
    assert site.count("/thumb/11") == 1
    site.uploads = []
    page.evaluate("document.dispatchEvent(new Event('visibilitychange'))")
    expect(page.get_by_test_id("upload-card")).to_have_count(0)
    expect(page.get_by_test_id("uploads-empty")).to_be_visible()


def test_selected_previews_survive_add_remove_and_upload(page, site):
    site.open()
    page.get_by_test_id("files").set_input_files({"name": "first.png", "mimeType": "image/png", "buffer": PNG})
    page.get_by_test_id("file-preview-media").evaluate("el => window.firstPreview = el")
    page.get_by_test_id("files").set_input_files({"name": "second.png", "mimeType": "image/png", "buffer": PNG})
    expect(page.get_by_test_id("file-preview")).to_have_count(2)
    assert page.evaluate("firstPreview === document.querySelector('[data-testid=file-preview-media]')")
    page.get_by_test_id("file-preview-media").last.evaluate("el => window.secondPreview = el")
    page.get_by_test_id("file-preview").and_(page.locator('[data-filename="first.png"]')).get_by_test_id("remove-file").click()
    assert page.evaluate("secondPreview === document.querySelector('[data-testid=file-preview-media]')")
    page.get_by_test_id("upload-submit").click()
    expect(page.get_by_test_id("file-preview")).to_have_count(0)
    expect(page.get_by_test_id("upload-error")).to_be_empty()
    expect(page.get_by_test_id("upload-card").first.get_by_test_id("upload-item")).to_have_attribute("data-status", "completed")
    assert site.count("/users/1/uploads.json") == 1
    assert site.count("/uploads/direct") == 1
    assert site.count("/uploads/2/items/20/complete") == 1


def test_visibility_pause_resume_and_logout(page, site):
    site.uploads = [upload(processing=True)]
    site.complete_after = 100
    site.open()
    page.evaluate("Object.defineProperty(document, 'hidden', {configurable: true, value: true}); document.dispatchEvent(new Event('visibilitychange'))")
    page.wait_for_timeout(2500)
    assert site.polls == 0
    page.evaluate("Object.defineProperty(document, 'hidden', {configurable: true, value: false}); document.dispatchEvent(new Event('visibilitychange'))")
    page.wait_for_function("document.querySelector('[data-testid=queue-status]').textContent !== ''")
    page.get_by_test_id("logout").click()
    expect(page.get_by_test_id("login-panel")).to_be_visible()
    requests = len(site.requests)
    page.wait_for_timeout(2500)
    assert len(site.requests) == requests
    expect(page.get_by_test_id("upload-card")).to_have_count(0)


def test_failed_url_retry_uses_response_and_only_polls_active_upload(page, site):
    failed = upload()
    failed["status"] = "error"
    failed["upload_media_assets"][0].update(status="error", source_url="http://source.test/image.png", post_id=None, error="Download failed")
    site.uploads = [failed]
    site.complete_after = 1
    site.open()
    page.get_by_test_id("retry-upload").click()
    expect(page.get_by_test_id("queue-status")).to_have_text("Processing in the background")
    expect(page.get_by_test_id("queue-status")).to_be_empty(timeout=10000)
    expect(page.get_by_test_id("upload-item")).to_have_attribute("data-status", "completed")
    assert site.count("/uploads/1/retry") == 1
    assert site.count("/users/1/uploads.json") == 1
    assert site.polls == 1


def test_shared_url_survives_session_restore_and_login_with_reactive_metadata(page, site):
    site.authenticated = False
    page.goto("http://upload.test/uploads/new?url=https%3A%2F%2Fsource.test%2Fimage.png")
    expect(page.get_by_test_id("login-panel")).to_be_visible()
    expect(page.get_by_test_id("source-url")).to_have_value("https://source.test/image.png")
    page.get_by_test_id("login").fill("owner")
    page.get_by_test_id("api-key").fill("key")
    page.get_by_test_id("sign-in").click()
    expect(page.get_by_test_id("workspace")).to_be_visible()
    expect(page.get_by_test_id("url-panel")).to_be_visible()
    expect(page.get_by_test_id("source-label")).to_be_hidden()
    page.get_by_test_id("tags").fill("test")
    expect(page.get_by_test_id("tag-suggestions")).to_be_visible()
    page.get_by_test_id("tag-suggestion").click()
    expect(page.get_by_test_id("tags")).to_have_value("test_tag ")
    page.get_by_test_id("rating-q").check()
    expect(page.get_by_test_id("rating-q")).to_be_checked()
    with page.expect_request(lambda request: request.method == "POST" and request.url.endswith("/uploads.json")) as received:
        page.get_by_test_id("upload-submit").click()
    assert received.value.post_data_json == {"upload": {"source": "https://source.test/image.png", "tag_string": "test_tag ", "rating": "q"}}
    expect(page.get_by_test_id("transfer-label")).to_have_text("Done.")
    expect(page.get_by_test_id("tags")).to_have_value("")
    expect(page.get_by_test_id("source-url")).to_have_value("")
    assert "?" not in page.url
    page.set_viewport_size({"width": 390, "height": 844})
    assert page.evaluate("document.documentElement.scrollWidth <= innerWidth")


def test_partial_transfer_retry_keeps_previews_and_reuses_batch(page, site):
    site.fail_storage_once = True
    site.open()
    page.get_by_test_id("files").set_input_files([
        {"name": "first.png", "mimeType": "image/png", "buffer": PNG},
        {"name": "second.png", "mimeType": "image/png", "buffer": PNG},
    ])
    page.get_by_test_id("file-preview-media").first.evaluate("el => window.firstPreview = el")
    page.get_by_test_id("upload-submit").click()
    expect(page.get_by_test_id("upload-error")).to_contain_text("retry unfinished files")
    expect(page.get_by_test_id("upload-submit")).to_be_enabled()
    assert page.evaluate("firstPreview === document.querySelector('[data-testid=file-preview-media]')")
    expect(page.get_by_test_id("file-preview")).to_have_count(2)
    page.get_by_test_id("upload-submit").click()
    expect(page.get_by_test_id("file-preview")).to_have_count(0)
    expect(page.get_by_test_id("upload-error")).to_be_empty()
    assert site.count("/uploads/direct") == 1
    assert site.count("/storage") == 3
    assert site.count("/uploads/2/items/20/complete") == 1
    assert site.count("/uploads/2/items/21/complete") == 1
    assert site.count("/users/1/uploads.json") == 1


def test_polling_failure_backs_off_and_recovers(page, site):
    site.uploads = [upload(processing=True)]
    site.complete_after = 1
    site.fail_polls = True
    site.open()
    expect(page.get_by_test_id("queue-status")).to_have_text("Temporarily unavailable", timeout=5000)
    assert site.polls == 1
    page.wait_for_timeout(2500)
    assert site.polls == 1  # First failure increases the next interval to four seconds.
    site.fail_polls = False
    expect(page.get_by_test_id("queue-status")).to_be_empty(timeout=6000)
    assert site.polls == 2
    assert site.count("/users/1/uploads.json") == 1


def test_cancelled_poll_cannot_overwrite_newer_reconciliation(page, site):
    site.uploads = [upload(processing=True)]
    held = []
    page.route("http://upload.test/uploads/1", lambda route: held.append(route))
    site.open()
    page.wait_for_timeout(2500)
    assert len(held) == 1
    old = copy.deepcopy(site.uploads[0])
    site.uploads = [upload()]
    page.evaluate("document.dispatchEvent(new Event('visibilitychange'))")
    expect(page.get_by_test_id("queue-status")).to_be_empty()
    held[0].fulfill(json=old)
    page.wait_for_timeout(100)
    expect(page.get_by_test_id("queue-status")).to_be_empty()
    expect(page.get_by_test_id("upload-item")).to_have_count(1)
