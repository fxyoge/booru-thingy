from urllib.parse import urlencode

from client import assert_original
from playwright.sync_api import expect


def sign_in(page, target):
    expect(page.get_by_test_id("login-panel")).to_be_visible()
    page.get_by_test_id("login").fill(target["login"])
    page.get_by_test_id("api-key").fill(target["key"])
    page.get_by_test_id("sign-in").click()
    expect(page.get_by_test_id("workspace")).to_be_visible()


def test_browser_upload_session_autocomplete_and_logout(page, api, media, tag, target):
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    fixture = media()
    page.goto(f"{target['base_url']}/uploads/new")
    sign_in(page, target)
    page.get_by_test_id("upload-submit").click()
    expect(page.get_by_test_id("upload-error")).to_contain_text("Choose at least one file")
    page.get_by_test_id("files").set_input_files(str(fixture))
    expect(page.get_by_test_id("file-preview")).to_have_count(1)
    page.get_by_test_id("tags").fill(tag)
    page.get_by_test_id("source").fill("https://example.invalid/browser")
    page.get_by_test_id("rating-q").check()
    expect(page.get_by_test_id("rating-q")).to_be_checked()
    with page.expect_response(lambda response: response.request.method == "POST" and response.url.endswith("/uploads/direct")) as response:
        page.get_by_test_id("upload-submit").click()
    accepted = response.value.json()
    result = api.await_upload(accepted["id"])
    card = page.get_by_test_id("upload-card").and_(page.locator(f'[data-upload-id="{accepted["id"]}"]'))
    expect(card).to_contain_text(fixture.name, timeout=30000)
    expect(card.get_by_test_id("upload-item")).to_have_attribute("data-status", "completed", timeout=30000)
    post = api.get(f"/posts/{result['upload_media_assets'][0]['post_id']}.json")
    assert post["tag_string"] == tag and post["rating"] == "q"
    assert post["source"] == "https://example.invalid/browser"
    assert_original(post, fixture)
    expect(page.get_by_test_id("upload-error")).to_be_empty()
    expect(page.get_by_test_id("file-preview")).to_have_count(0)
    page.reload()
    expect(page.get_by_test_id("workspace")).to_be_visible()
    page.get_by_test_id("tags").fill(tag[:-3])
    expect(page.get_by_test_id("tag-suggestions")).to_be_visible()
    page.get_by_test_id("tag-suggestion").and_(page.locator(f'[data-tag="{tag}"]')).click()
    expect(page.get_by_test_id("tags")).to_have_value(f"{tag} ")
    page.set_viewport_size({"width": 390, "height": 844})
    assert page.evaluate("document.documentElement.scrollWidth <= innerWidth"), "mobile overflow"
    page.emulate_media(color_scheme="dark")
    expect(page.get_by_test_id("upload-submit")).to_be_visible()
    page.get_by_test_id("logout").click()
    expect(page.get_by_test_id("login-panel")).to_be_visible()
    page.reload()
    expect(page.get_by_test_id("login-panel")).to_be_visible()
    assert not errors, errors


def test_browser_shared_url_import(page, api, media, tag, target, sources):
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    fixture = media()
    source = sources(fixture.read_bytes())["redirect"]
    page.goto(f"{target['base_url']}/uploads/new?{urlencode({'url': source})}")
    sign_in(page, target)
    expect(page.get_by_test_id("source-url")).to_have_value(source)
    expect(page.get_by_test_id("url-panel")).to_be_visible()
    assert "?" not in page.url
    page.get_by_test_id("tags").fill(tag)
    with page.expect_response(lambda response: response.request.method == "POST" and response.url.endswith("/uploads.json")) as response:
        page.get_by_test_id("upload-submit").click()
    result = api.await_upload(response.value.json()["id"])
    post = api.get(f"/posts/{result['upload_media_assets'][0]['post_id']}.json")
    assert post["tag_string"] == tag and post["source"] == source
    assert_original(post, fixture)
    expect(page.get_by_test_id("upload-error")).to_be_empty()
    assert not errors, errors


def test_failed_file_requires_reupload_instead_of_processing_retry(page, api, tag, target):
    upload = api.upload([("corrupt.png", b"not media")], tag)
    api.await_upload(upload["id"], status="error")
    page.goto(f"{target['base_url']}/uploads/new")
    sign_in(page, target)
    card = page.get_by_test_id("upload-card").and_(page.locator(f'[data-upload-id="{upload["id"]}"]'))
    expect(card).to_contain_text("Select the file again to re-upload.")
    expect(card.get_by_test_id("retry-upload")).to_be_hidden()


def test_browser_partial_transfer_retry_preserves_batch_and_keeps_credentials_off_storage(page, api, media, tag, target):
    fixtures = [media(), media("jpg")]
    reservations = []
    storage_requests = []

    def storage(route):
        headers = route.request.all_headers()
        assert "authorization" not in headers
        assert "cookie" not in headers
        assert "x-csrf-token" not in headers
        storage_requests.append(route.request)
        if len(storage_requests) == 1:
            route.abort("failed")
        else:
            route.continue_()

    def response_received(response):
        if response.request.method == "POST" and response.url.endswith("/uploads/direct"):
            reservations.append(response.json())

    page.route("**/e2e", storage)
    page.on("response", response_received)
    page.goto(f"{target['base_url']}/uploads/new")
    sign_in(page, target)
    page.get_by_test_id("files").set_input_files([str(fixture) for fixture in fixtures])
    page.get_by_test_id("tags").fill(tag)
    page.get_by_test_id("upload-submit").click()
    expect(page.get_by_test_id("upload-error")).to_contain_text("retry unfinished files", timeout=30000)
    assert len(reservations) == 1
    # Retry reuses the original batch, skips the accepted item, and transfers only one file.
    page.get_by_test_id("upload-submit").click()
    expect(page.get_by_test_id("file-preview")).to_have_count(0, timeout=30000)
    expect(page.get_by_test_id("upload-error")).to_be_empty()
    assert len(reservations) == 1
    assert len(storage_requests) == 3
    result = api.await_upload(reservations[0]["id"])
    by_name = {item["filename"]: item for item in result["upload_media_assets"]}
    for fixture in fixtures:
        assert_original(api.get(f"/posts/{by_name[fixture.name]['post_id']}.json"), fixture)
    assert api.count(tag) == 2
