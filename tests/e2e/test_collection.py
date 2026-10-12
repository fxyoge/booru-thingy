from concurrent.futures import ThreadPoolExecutor
from threading import Barrier

from client import Client, assert_original


def ids(posts):
    return [post["id"] for post in posts]


def test_search_pagination_counts_and_edits(api, media, tag):
    first = api.publish(media(), f"{tag} {tag}_red")
    second = api.publish(media(), f"{tag} {tag}_blue", rating="q")
    third = api.publish(media(), f"{tag} {tag}_red")
    assert ids(api.posts(tag)) == [third["id"], second["id"], first["id"]]
    assert ids(api.posts(f"{tag} order:id_asc")) == [first["id"], second["id"], third["id"]]
    assert ids(api.posts(f"{tag} -{tag}_red")) == [second["id"]]
    assert ids(api.posts(f"{tag} rating:q")) == [second["id"]]
    assert ids(api.posts(f"{tag} md5:{first['md5']}")) == [first["id"]]
    assert ids(api.posts(f"{tag} id:{first['id']},{third['id']}")) == [third["id"], first["id"]]
    assert api.count(tag, limit=1) == 3
    pages = [api.posts(tag, limit=1, page=page) for page in range(1, 5)]
    assert [p[0]["id"] for p in pages[:3]] == [third["id"], second["id"], first["id"]]
    assert pages[3] == []
    assert ids(api.posts(tag, page=f"b{second['id']}")) == [first["id"]]
    assert ids(api.posts(tag, page=f"a{second['id']}")) == [third["id"]]

    api.edit(first["id"], tag_string=f"{tag} {tag}_blue", rating="e", source="https://example.invalid/edited")
    edited = api.get(f"/posts/{first['id']}.json")
    assert edited["source"] == "https://example.invalid/edited"
    assert edited["rating"] == "e"
    assert set(edited["tag_string"].split()) == {tag, f"{tag}_blue"}
    assert ids(api.posts(f"{tag} rating:e")) == [first["id"]]
    assert ids(api.posts(f"{tag} source:edited")) == [first["id"]]
    assert api.count(f"{tag}_red") == 1
    assert api.count(f"{tag}_blue") == 2
    tags = api.get("/tags.json", **{"search[name_comma]": f"{tag}_red,{tag}_blue"})
    assert {t["name"]: t["post_count"] for t in tags} == {f"{tag}_red": 1, f"{tag}_blue": 2}
    suggestions = api.get("/autocomplete.json", **{"search[query]": f"{tag}_b"})
    assert [t["value"] for t in suggestions] == [f"{tag}_blue"]


def test_parent_relationships_and_atomic_invalid_edit(api, media, tag):
    parent = api.publish(media(), tag)
    child = api.publish(media(), tag)
    api.edit(child["id"], parent_id=parent["id"])
    assert api.get(f"/posts/{child['id']}.json")["parent_id"] == parent["id"]
    assert api.get(f"/posts/{parent['id']}.json")["has_children"] is True
    assert ids(api.posts(f"{tag} parent:{parent['id']}")) == [child["id"]]
    assert ids(api.posts(f"{tag} parent:none")) == [parent["id"]]
    api.edit(child["id"], expected=400, parent_id=9223372036854775807, tag_string=f"{tag}_rollback", rating="e")
    unchanged = api.get(f"/posts/{child['id']}.json")
    assert unchanged["tag_string"] == tag
    assert unchanged["rating"] == "g"
    assert unchanged["parent_id"] == parent["id"]
    assert api.count(f"{tag}_rollback") == 0
    api.edit(parent["id"], expected=400, parent_id=child["id"])
    api.edit(child["id"], parent_id="")
    assert api.get(f"/posts/{parent['id']}.json")["has_children"] is False


def test_favorites_are_idempotent_and_removable(api, media, tag, target):
    post = api.publish(media(), tag)
    for _ in range(2):
        api.request("POST", "/favorites.json", data={"post_id": post["id"]})
    assert api.get(f"/posts/{post['id']}.json")["fav_count"] == 1
    favorites = api.get("/favorites.json", **{"search[post_id]": post["id"]})
    assert [favorite["post_id"] for favorite in favorites] == [post["id"]]
    assert ids(api.posts(f"{tag} fav:{target['login']}")) == [post["id"]]
    assert api.count(f"{tag} fav:{target['login']}") == 1
    api.request("DELETE", f"/favorites/{post['id']}.json")
    assert api.get(f"/posts/{post['id']}.json")["fav_count"] == 0
    assert api.posts(f"{tag} fav:{target['login']}") == []
    assert api.get("/favorites.json", **{"search[post_id]": post["id"]}) == []


def test_sequential_duplicate_preserves_edits(api, media, tag):
    fixture = media()
    post = api.publish(fixture, tag)
    api.edit(post["id"], tag_string=f"{tag} {tag}_edited", rating="q", source="edited")
    upload = api.upload([(fixture.name, fixture.read_bytes())], f"{tag}_overwrite", rating="e")
    result = api.await_upload(upload["id"])
    item = result["upload_media_assets"][0]
    assert item["duplicate"] is True
    assert item["post_id"] == post["id"]
    current = api.get(f"/posts/{post['id']}.json")
    assert set(current["tag_string"].split()) == {tag, f"{tag}_edited"}
    assert current["rating"] == "q" and current["source"] == "edited"
    assert api.count(tag) == 1
    assert api.count(f"{tag}_overwrite") == 0
    assert_original(current, fixture)


def test_concurrent_duplicate_uploads_publish_one_post(api, media, tag, target):
    fixture = media()
    ready = Barrier(4)

    def submit(_):
        client = Client(**target)
        try:
            ready.wait(timeout=20)
            return client.upload([(fixture.name, fixture.read_bytes())], tag)
        finally:
            client.http.close()

    with ThreadPoolExecutor(max_workers=4) as executor:
        accepted = list(executor.map(submit, range(4)))
    results = [api.await_upload(upload["id"]) for upload in accepted]
    items = [result["upload_media_assets"][0] for result in results]
    assert len({item["post_id"] for item in items}) == 1
    assert sum(item["duplicate"] for item in items) == 3
    assert api.count(tag) == 1
    assert_original(api.posts(tag)[0], fixture)
