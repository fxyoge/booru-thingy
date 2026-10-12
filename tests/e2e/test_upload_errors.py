import pytest
from client import assert_original


def test_batch_maps_each_file_to_its_post(api, media, tag):
    fixtures = [media("png"), media("jpg"), media("webp")]
    upload = api.upload([(f.name, f.read_bytes()) for f in fixtures], tag)
    result = api.await_upload(upload["id"])
    items = {item["filename"]: item for item in result["upload_media_assets"]}
    assert set(items) == {f.name for f in fixtures}
    assert result["media_asset_count"] == 3
    assert api.count(tag) == 3
    for fixture in fixtures:
        item = items[fixture.name]
        assert item["status"] == "completed" and item["duplicate"] is False
        assert_original(api.get(f"/posts/{item['post_id']}.json"), fixture)


def test_mixed_batch_preserves_successful_items(api, media, tag):
    fixture = media()
    upload = api.upload([(fixture.name, fixture.read_bytes()), ("corrupt.png", b"not media")], tag)
    result = api.await_upload(upload["id"], status="error")
    items = {item["filename"]: item for item in result["upload_media_assets"]}
    assert items["corrupt.png"]["status"] == "error"
    assert "unsupported media format" in items["corrupt.png"]["error"]
    assert items["corrupt.png"]["post_id"] is None
    assert items[fixture.name]["status"] == "completed"
    assert_original(api.get(f"/posts/{items[fixture.name]['post_id']}.json"), fixture)
    assert api.count(tag) == 1
    assert api.publish(media(), tag)["id"] != items[fixture.name]["post_id"]
    assert api.count(tag) == 2


@pytest.mark.parametrize("case", ["empty", "invalid_rating", "invalid_tags", "too_many", "oversized"])
def test_rejected_upload_does_not_publish(api, media, tag, case):
    content = media().read_bytes()
    files = [("image.png", content)]
    tags = tag
    rating, expected = "g", 400
    if case == "empty":
        files = [("empty.png", b"")]
    elif case == "invalid_rating":
        rating = "invalid"
    elif case == "invalid_tags":
        tags = f"{tag} invalid:tag"
    elif case == "too_many":
        files *= 33
    else:
        # Discover the same public upload limit that the browser uses.
        from html.parser import HTMLParser

        class LimitParser(HTMLParser):
            limit = None

            def handle_starttag(self, name, attrs):
                if name == "body":
                    self.limit = int(dict(attrs)["data-max-bytes"])

        parser = LimitParser()
        parser.feed(api.request("GET", "/uploads/new").text)
        assert parser.limit is not None and parser.limit <= 16 * 1024 * 1024, "Use a disposable E2E target with a small upload limit"
        files = [("large.png", b"x" * (parser.limit + 1))]
        expected = 413
    error = api.upload(files, tags, rating=rating, expected=expected)
    assert error["success"] is False
    reason = {
        "empty": "empty files",
        "invalid_rating": "rating must be",
        "invalid_tags": "invalid tag string",
        "too_many": "maximum 32 files",
        "oversized": "exceeds configured size limit",
    }[case]
    assert reason in error["reason"], error
    assert api.count(tag) == 0


def test_missing_resources_and_invalid_url(api, tag):
    for path in ["/posts/9223372036854775807.json", "/uploads/9223372036854775807.json"]:
        api.request("GET", path, expected=404)
    for url in ["", "file:///etc/passwd", "https://user:password@example.invalid/image.png"]:
        api.import_url(url, tag, expected=400)
    assert api.count(tag) == 0
