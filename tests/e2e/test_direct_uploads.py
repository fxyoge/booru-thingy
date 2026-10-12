"""Exercise signed capabilities through the public API and actual storage gateway."""
import base64
import concurrent.futures
import json
import time
from datetime import UTC, datetime

import httpx
import pytest
from client import Client, assert_original, send_ticket


def reserved(api, content=b"not media"):
    upload = api.reserve([("image.png", content)])
    item = upload["upload_media_assets"][0]
    return upload["id"], item["id"], api.ticket(upload["id"], item["id"])


@pytest.mark.parametrize("mutation", ["key", "signature", "policy", "content_type", "acl"])
def test_signed_policy_cannot_be_widened(api, mutation):
    upload, item, ticket = reserved(api)
    fields = dict(ticket["fields"])
    if mutation == "key":
        fields["key"] = "media/unauthorized/original.png"
    elif mutation == "signature":
        fields["x-amz-signature"] = "0" * 64
    elif mutation == "policy":
        policy = json.loads(base64.b64decode(fields["policy"]))
        policy["expiration"] = "2099-01-01T00:00:00Z"
        fields["policy"] = base64.b64encode(json.dumps(policy).encode()).decode()
    elif mutation == "acl":
        fields["acl"] = "public-read"
    else:
        fields["Content-Type"] = "text/html"
    response = send_ticket(ticket, "image.png", b"not media", fields)
    assert response.status_code in (400, 403), response.text
    api.complete(upload, item, expected=409)


@pytest.mark.parametrize("content", [b"", b"short", b"not media!", b"x" * 65536])
def test_storage_enforces_exact_reserved_size(api, content):
    upload, item, ticket = reserved(api)
    response = send_ticket(ticket, "image.png", content)
    assert response.status_code in (400, 403), response.text
    api.complete(upload, item, expected=409)


def test_completion_requires_a_matching_reservation(api):
    first, item, ticket = reserved(api)
    second, other, _ = reserved(api)
    api.complete(first, item, expected=409)  # No object, no job.
    api.complete(second, item, expected=404)
    api.ticket(first, other, expected=404)
    assert send_ticket(ticket, "image.png", b"not media").status_code == 204


def test_superseded_tickets_do_not_select_the_old_object(api):
    upload, item, old = reserved(api)
    latest = api.ticket(upload, item)
    assert latest["fields"]["key"] != old["fields"]["key"]
    assert send_ticket(old, "image.png", b"not media").status_code == 204
    api.complete(upload, item, expected=409)  # Only the newest reservation is selected.
    assert send_ticket(latest, "image.png", b"not media").status_code == 204
    api.complete(upload, item)


def test_expired_ticket_rejects_transfer_and_can_be_replaced(api):
    upload, item, ticket = reserved(api)
    remaining = (datetime.fromisoformat(ticket["expires_at"]) - datetime.now(UTC)).total_seconds()
    assert remaining <= 20, "Use a disposable E2E target with UPLOAD_TICKET_TTL_SECONDS=15"
    time.sleep(max(0, remaining) + 1)
    response = send_ticket(ticket, "image.png", b"not media")
    assert response.status_code in (400, 403), response.text
    api.complete(upload, item, expected=409)
    replacement = api.ticket(upload, item)
    assert send_ticket(replacement, "image.png", b"not media").status_code == 204
    api.complete(upload, item)


def test_completion_is_concurrent_and_idempotent_and_replay_cannot_change_media(api, media, tag, target):
    fixture = media()
    content = fixture.read_bytes()
    upload = api.reserve([(fixture.name, content)], tag)
    item = upload["upload_media_assets"][0]["id"]
    ticket = api.ticket(upload["id"], item)
    assert send_ticket(ticket, fixture.name, content).status_code == 204

    def complete(_):
        client = Client(target["base_url"], target["login"], target["key"])
        try:
            return client.complete(upload["id"], item)
        finally:
            client.http.close()

    with concurrent.futures.ThreadPoolExecutor(max_workers=4) as executor:
        list(executor.map(complete, range(4)))
    # A valid, unexpired grant can still overwrite incoming, but not the snapshot.
    assert send_ticket(ticket, fixture.name, b"x" * len(content)).status_code == 204
    result = api.await_upload(upload["id"])
    post = api.get(f"/posts/{result['upload_media_assets'][0]['post_id']}.json")
    assert_original(post, fixture)
    api.complete(upload["id"], item)
    api.ticket(upload["id"], item, expected=409)
    assert api.count(tag) == 1


def test_reservation_checks_aggregate_limit_and_rejects_overflow(api):
    response = api.request("POST", "/uploads/direct", expected=413, json={
        "files": [{"filename": "one", "size": 2**63}, {"filename": "two", "size": 2**63}]
    })
    assert "exceeds configured size limit" in response.json()["reason"]


def test_small_api_body_limit_including_unauthenticated_login(target):
    with httpx.Client(base_url=target["base_url"], trust_env=False) as http:
        response = http.post("/login", json={"login": "x" * 17000, "api_key": "wrong"})
        assert response.status_code == 413


def test_basic_auth_mutations_reject_foreign_origin(api):
    api.request("POST", "/uploads/direct", expected=403,
                headers={"Origin": "https://foreign.invalid"},
                json={"files": [{"filename": "image.png", "size": 9}]})


def test_storage_cors_is_limited_to_application_origin(api, target):
    _, _, ticket = reserved(api)
    with httpx.Client(trust_env=False) as http:
        for origin, allowed in [(target["base_url"], True), ("https://foreign.invalid", False)]:
            try:
                response = http.options(ticket["url"], headers={
                    "Origin": origin, "Access-Control-Request-Method": "POST",
                    "Access-Control-Request-Headers": "content-type",
                })
            except httpx.RemoteProtocolError:
                # SeaweedFS 4.48's denied-origin handler closes the request rather
                # than returning a valid HTTP error. It must never do this for our origin.
                assert not allowed
                continue
            assert (response.headers.get("access-control-allow-origin") == origin) is allowed
        response = http.get(ticket["url"])
        assert response.status_code == 403, "anonymous bucket listing must not be allowed"
