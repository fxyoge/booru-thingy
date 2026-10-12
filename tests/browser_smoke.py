"""Real browser upload smoke test against a running, disposable test instance.

Run with Python + Playwright: python tests/browser_smoke.py
TEST_BASE_URL, OWNER_LOGIN, OWNER_API_KEY configure the target.
"""
import os
import subprocess
import tempfile
from pathlib import Path
from playwright.sync_api import sync_playwright, expect

base = os.environ.get("TEST_BASE_URL", "http://localhost:8080")
login = os.environ.get("OWNER_LOGIN", "owner")
key = os.environ["OWNER_API_KEY"]

with tempfile.TemporaryDirectory() as temp, sync_playwright() as playwright:
    fixture = Path(temp) / "browser-fixture.png"
    subprocess.run([
        "ffmpeg", "-v", "error", "-y", "-f", "lavfi", "-i",
        "color=c=orange:s=160x90", "-frames:v", "1", "-threads", "1", str(fixture)
    ], check=True)
    browser = playwright.chromium.launch()
    page = browser.new_page(viewport={"width": 1280, "height": 1000})
    errors = []
    page.on("pageerror", lambda error: errors.append(str(error)))
    page.goto(f"{base}/uploads/new")
    expect(page.locator("#login-panel")).to_be_visible()
    page.locator("[name=login]").fill(login)
    page.locator("[name=api_key]").fill(key)
    page.get_by_role("button", name="Sign in", exact=False).click()
    expect(page.locator("#workspace")).to_be_visible()
    page.locator("#files").set_input_files(str(fixture))
    expect(page.locator(".preview")).to_have_count(1)
    page.locator("#tags").fill("browser_test orange")
    page.get_by_role("button", name="Upload & keep", exact=False).click()
    expect(page.locator("#queue")).to_contain_text("browser-fixture.png", timeout=30000)
    expect(page.locator("#queue")).to_contain_text("Published", timeout=30000)
    expect(page.locator("#upload-error")).to_be_empty()
    expect(page.locator("#previews .preview")).to_have_count(0)
    page.locator("#tags").fill("bro")
    expect(page.locator("#suggestions")).to_be_visible()
    page.locator("#suggestions button").first.click()
    expect(page.locator("#tags")).to_have_value("browser_test ")
    page.set_viewport_size({"width": 390, "height": 844})
    assert page.evaluate("document.documentElement.scrollWidth <= innerWidth"), "mobile overflow"
    page.emulate_media(color_scheme="dark")
    expect(page.locator("#upload-button")).to_be_visible()
    page.goto(f"{base}/uploads/new?url=https%3A%2F%2Fexample.com%2Fimage.png")
    expect(page.locator("#source-url")).to_have_value("https://example.com/image.png")
    expect(page.locator("#url-panel")).to_be_visible()
    assert "?" not in page.url, "shared URL should be removed from browser history"
    page.get_by_role("button", name="Sign out").click()
    expect(page.locator("#login-panel")).to_be_visible()
    assert not errors, errors
    browser.close()
print("Browser smoke test passed: login, upload/publication, autocomplete, mobile/dark layout, sharing, logout.")
