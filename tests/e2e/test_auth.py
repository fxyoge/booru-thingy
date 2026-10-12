import httpx
import pytest


def test_api_authentication(api, target):
    with httpx.Client(base_url=target["base_url"], timeout=20, trust_env=False) as http:
        assert http.get("/posts.json").status_code == 401
        assert http.get("/posts.json", auth=(target["login"], "wrong")).status_code == 401
        response = http.get("/profile.json", params={"login": target["login"], "api_key": target["key"]})
        assert response.status_code == 200, response.text
        assert response.json()["id"] == 1
    assert api.get("/profile.json")["id"] == 1


def test_browser_session_csrf_and_logout(api, media, tag, target):
    post = api.publish(media(), tag)
    with httpx.Client(base_url=target["base_url"], timeout=20, trust_env=False) as http:
        credentials = {"login": target["login"], "api_key": target["key"]}
        assert http.post("/login", json={**credentials, "api_key": "wrong"}).status_code == 401
        assert http.post("/login", json=credentials, headers={"origin": "https://another.invalid"}).status_code == 403
        response = http.post("/login", json=credentials, headers={"origin": target["base_url"]})
        assert response.status_code == 200, response.text
        cookie = http.cookies.get("booru_session")
        assert cookie
        assert "HttpOnly" in response.headers["set-cookie"]
        csrf = response.json()["csrf_token"]
        assert http.get("/session").json()["csrf_token"] == csrf
        for headers in [{}, {"x-csrf-token": "wrong"}]:
            rejected = http.post("/favorites.json", data={"post_id": post["id"]}, headers=headers)
            assert rejected.status_code == 403, rejected.text
        assert api.get(f"/posts/{post['id']}.json")["fav_count"] == 0
        response = http.post("/favorites.json", data={"post_id": post["id"]}, headers={"x-csrf-token": csrf})
        assert response.status_code == 200, response.text
        assert api.get(f"/posts/{post['id']}.json")["fav_count"] == 1
        assert http.delete("/session", headers={"x-csrf-token": csrf}).status_code == 200
        # Replay the old cookie to prove server-side invalidation, not just deletion.
        assert http.get("/session", headers={"cookie": f"booru_session={cookie}"}).status_code == 401


@pytest.mark.parametrize("method,path", [("POST", "/uploads.json"), ("POST", "/uploads/direct"), ("POST", "/uploads/1/items/1/ticket"), ("POST", "/uploads/1/items/1/complete"), ("PUT", "/posts/1.json"), ("POST", "/favorites.json")])
def test_unauthenticated_mutations_are_rejected(target, method, path):
    with httpx.Client(base_url=target["base_url"], timeout=20, trust_env=False) as http:
        response = http.request(method, path, json={})
        assert response.status_code == 401, response.text
