import pytest
from client import assert_jpeg_variant, assert_original, download


@pytest.mark.parametrize("extension,animated", [
    ("jpg", False), ("png", False), ("gif", True), ("webp", False),
    ("webp", True), ("mp4", False), ("webm", False),
])
def test_supported_media_roundtrip(api, media, tag, extension, animated):
    fixture = media(extension, animated=animated)
    post = api.publish(fixture, f"{tag} roundtrip", rating="q", source="https://example.invalid/art")
    assert post["file_ext"] == extension
    assert (post["image_width"], post["image_height"]) == (320, 240)
    assert set(post["tag_string"].split()) == {tag, "roundtrip"}
    assert post["rating"] == "q"
    assert post["source"] == "https://example.invalid/art"
    assert post["tag_count_general"] == 2
    assert_original(post, fixture)
    assert download(post["file_url"]).headers["content-type"].split(";")[0] == {
        "jpg": "image/jpeg", "png": "image/png", "gif": "image/gif",
        "webp": "image/webp", "mp4": "video/mp4", "webm": "video/webm",
    }[extension]
    variants = {v["type"]: v for v in post["media_asset"]["variants"]}
    assert set(variants) == {"original", "full", "180x180", "360x360", "720x720", "sample"}
    for kind, bound in [("180x180", 180), ("360x360", 360), ("720x720", 720)]:
        width, height = assert_jpeg_variant(variants[kind])
        assert 0 < width <= min(bound, 320)
        assert 0 < height <= min(bound, 240)
        assert abs(width / height - 4 / 3) < 0.02
    if animated or extension in ("mp4", "webm"):
        assert post["media_asset"]["duration"] == pytest.approx(0.4, abs=0.12)
        assert download(variants["sample"]["url"]).content == fixture.read_bytes()
    else:
        assert_jpeg_variant(variants["sample"])
    assert api.posts(tag)[0]["id"] == post["id"]
    assert api.count(tag) == 1


def test_large_image_resizes_without_changing_original(api, media, tag):
    fixture = media(size=(2400, 1800))
    post = api.publish(fixture, tag)
    assert_original(post, fixture)
    assert (post["image_width"], post["image_height"]) == (2400, 1800)
    variants = {v["type"]: v for v in post["media_asset"]["variants"]}
    for kind, width in [("180x180", 180), ("360x360", 360), ("720x720", 720), ("sample", 1600)]:
        assert assert_jpeg_variant(variants[kind]) == (width, width * 3 // 4)


def test_exif_orientation_is_reflected_in_metadata_and_preview(api, media, tag):
    fixture = media("jpg", oriented=True)
    post = api.publish(fixture, tag)
    assert_original(post, fixture)
    assert (post["image_width"], post["image_height"]) == (240, 320)
    preview = next(v for v in post["media_asset"]["variants"] if v["type"] == "180x180")
    assert assert_jpeg_variant(preview) == (135, 180)


def test_default_metadata(api, media):
    post = api.publish(media())
    assert post["tag_string"] == "tagme"
    assert post["rating"] == "g"
