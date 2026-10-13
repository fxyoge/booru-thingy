from urllib.parse import urlencode

from client import assert_original
from playwright.sync_api import expect


def sign_in(page, target):
    expect(page.locator("#login-panel")).to_be_visible()
    page.locator("[name=login]").fill(target["login"])
    page.locator("[name=api_key]").fill(target["key"])
    page.get_by_role("button", name="Sign in", exact=False).click()
    expect(page.locator("#workspace")).to_be_visible()


def test_browser_upload_session_autocomplete_and_logout(page, api, media, tag, target):
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    fixture = media()
    page.goto(f"{target['base_url']}/uploads/new")
    sign_in(page, target)
    page.get_by_role("button", name="Upload & keep", exact=False).click()
    expect(page.locator("#upload-error")).to_contain_text("Choose at least one file")
    page.locator("#files").set_input_files(str(fixture))
    expect(page.locator("#previews .preview")).to_have_count(1)
    page.locator("#tags").fill(tag)
    page.locator("#source").fill("https://example.invalid/browser")
    page.get_by_text("Questionable", exact=True).click()
    expect(page.locator('[name=rating][value=q]')).to_be_checked()
    with page.expect_response(lambda response: response.request.method == "POST" and response.url.endswith("/uploads/direct")) as response:
        page.get_by_role("button", name="Upload & keep", exact=False).click()
    accepted = response.value.json()
    result = api.await_upload(accepted["id"])
    card = page.locator(".queue-card").filter(has=page.get_by_text(f"Upload #{accepted['id']}", exact=True))
    expect(card).to_contain_text(fixture.name, timeout=30000)
    expect(card).to_contain_text("Published", timeout=30000)
    post = api.get(f"/posts/{result['upload_media_assets'][0]['post_id']}.json")
    assert post["tag_string"] == tag and post["rating"] == "q"
    assert post["source"] == "https://example.invalid/browser"
    assert_original(post, fixture)
    expect(page.locator("#upload-error")).to_be_empty()
    expect(page.locator("#previews .preview")).to_have_count(0)
    page.reload()
    expect(page.locator("#workspace")).to_be_visible()
    page.locator("#tags").fill(tag[:-3])
    expect(page.locator("#suggestions")).to_be_visible()
    page.locator("#suggestions button").filter(has_text=tag).click()
    expect(page.locator("#tags")).to_have_value(f"{tag} ")
    page.set_viewport_size({"width": 390, "height": 844})
    assert page.evaluate("document.documentElement.scrollWidth <= innerWidth"), "mobile overflow"
    page.emulate_media(color_scheme="dark")
    expect(page.locator("#upload-button")).to_be_visible()
    page.get_by_role("button", name="Sign out").click()
    expect(page.locator("#login-panel")).to_be_visible()
    page.reload()
    expect(page.locator("#login-panel")).to_be_visible()
    assert not errors, errors


def test_browser_shared_url_import(page, api, media, tag, target, sources):
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    fixture = media()
    source = sources(fixture.read_bytes())["redirect"]
    page.goto(f"{target['base_url']}/uploads/new?{urlencode({'url': source})}")
    sign_in(page, target)
    expect(page.locator("#source-url")).to_have_value(source)
    expect(page.locator("#url-panel")).to_be_visible()
    assert "?" not in page.url
    page.locator("#tags").fill(tag)
    with page.expect_response(lambda response: response.request.method == "POST" and response.url.endswith("/uploads.json")) as response:
        page.get_by_role("button", name="Upload & keep", exact=False).click()
    result = api.await_upload(response.value.json()["id"])
    post = api.get(f"/posts/{result['upload_media_assets'][0]['post_id']}.json")
    assert post["tag_string"] == tag and post["source"] == source
    assert_original(post, fixture)
    expect(page.locator("#upload-error")).to_be_empty()
    assert not errors, errors


def test_failed_file_requires_reupload_instead_of_processing_retry(page, api, tag, target):
    upload = api.upload([("corrupt.png", b"not media")], tag)
    api.await_upload(upload["id"], status="error")
    page.goto(f"{target['base_url']}/uploads/new")
    sign_in(page, target)
    card = page.locator(".queue-card").filter(has=page.get_by_text(f"Upload #{upload['id']}", exact=True))
    expect(card).to_contain_text("Select the file again to re-upload.")
    expect(card.get_by_role("button", name="Retry failed items")).to_have_count(0)


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
    page.locator("#files").set_input_files([str(fixture) for fixture in fixtures])
    page.locator("#tags").fill(tag)
    page.get_by_role("button", name="Upload & keep", exact=False).click()
    expect(page.locator("#upload-error")).to_contain_text("retry unfinished files", timeout=30000)
    assert len(reservations) == 1
    # Retry reuses the original batch, skips the accepted item, and transfers only one file.
    page.get_by_role("button", name="Upload & keep", exact=False).click()
    expect(page.locator("#previews .preview")).to_have_count(0, timeout=30000)
    expect(page.locator("#upload-error")).to_be_empty()
    assert len(reservations) == 1
    assert len(storage_requests) == 3
    result = api.await_upload(reservations[0]["id"])
    by_name = {item["filename"]: item for item in result["upload_media_assets"]}
    for fixture in fixtures:
        assert_original(api.get(f"/posts/{by_name[fixture.name]['post_id']}.json"), fixture)
    assert api.count(tag) == 2
