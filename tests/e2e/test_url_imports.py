import pytest
from client import assert_original


@pytest.mark.parametrize("route", ["url", "redirect"])
def test_url_import_roundtrip(api, media, tag, sources, route):
    fixture = media()
    registered = sources(fixture.read_bytes())
    url = registered[route]
    accepted = api.import_url(url, tag)
    result = api.await_upload(accepted["id"])
    item = result["upload_media_assets"][0]
    assert item["source_url"] == url
    assert item["duplicate"] is False
    post = api.get(f"/posts/{item['post_id']}.json")
    assert post["source"] == url
    assert post["tag_string"] == tag
    assert_original(post, fixture)
    assert api.count(tag) == 1


@pytest.mark.parametrize("route", ["missing", "empty", "invalid", "oversized", "chunked_oversized"])
def test_source_failure_is_reported_without_publication(api, media, tag, sources, route):
    registered = sources(media().read_bytes())
    accepted = api.import_url(registered[route], tag)
    result = api.await_upload(accepted["id"], status="error")
    item = result["upload_media_assets"][0]
    assert item["status"] == "error" and item["error"]
    reason = {
        "missing": "source returned HTTP 404",
        "empty": "source returned an empty file",
        "invalid": "unsupported media format",
        "oversized": "source exceeds upload limit",
        "chunked_oversized": "source exceeds upload limit",
    }[route]
    assert reason in item["error"], result
    assert item["post_id"] is None
    assert api.count(tag) == 0


def test_user_can_retry_a_repaired_source(api, media, tag, sources):
    registered = sources(b"not media")
    accepted = api.import_url(registered["url"], tag)
    api.await_upload(accepted["id"], status="error")
    assert api.count(tag) == 0
    fixture = media()
    sources(fixture.read_bytes(), name=registered["name"])
    api.request("POST", f"/uploads/{accepted['id']}/retry")
    result = api.await_upload(accepted["id"])
    assert_original(api.get(f"/posts/{result['upload_media_assets'][0]['post_id']}.json"), fixture)
    assert api.count(tag) == 1
