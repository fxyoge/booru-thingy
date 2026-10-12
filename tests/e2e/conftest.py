import os
import subprocess
import uuid

import httpx
import pytest
from client import Client
from PIL import Image


@pytest.fixture(scope="session")
def target():
    return {
        "base_url": os.environ.get("TEST_BASE_URL", "http://localhost:8080"),
        "login": os.environ.get("OWNER_LOGIN", "owner"),
        "key": os.environ["OWNER_API_KEY"],
    }


@pytest.fixture
def api(target):
    client = Client(**target)
    yield client
    client.http.close()


@pytest.fixture
def tag():
    return f"e2e_{uuid.uuid4().hex}"


@pytest.fixture
def media(tmp_path):
    def generate(extension="png", size=(320, 240), animated=False, oriented=False):
        identity = uuid.uuid4().hex
        path = tmp_path / f"{identity}.{extension}"
        width, height = size
        # A unique visible color survives lossy encoding, avoiding cross-test dedup.
        color = identity[:6]
        source = f"testsrc2=s={width}x{height}:r=10:d=0.4,drawbox=x=0:y=0:w=iw/2:h=ih/2:color=0x{color}:t=fill"
        args = ["ffmpeg", "-v", "error", "-nostdin", "-y", "-f", "lavfi", "-i", source, "-threads", "1"]
        if extension not in ("mp4", "webm") and not animated:
            args += ["-frames:v", "1"]
        if extension == "webp":
            args += ["-c:v", "libwebp_anim" if animated else "libwebp", "-lossless", "1"]
        elif extension == "mp4":
            args += ["-c:v", "libx264", "-pix_fmt", "yuv420p"]
        elif extension == "webm":
            args += ["-c:v", "libvpx-vp9", "-deadline", "realtime"]
        result = subprocess.run(args + [str(path)], check=False, capture_output=True, timeout=30)
        assert result.returncode == 0, result.stderr.decode(errors="replace")
        if oriented:
            with Image.open(path) as image:
                exif = image.getexif()
                exif[274] = 6  # Rotate 90 degrees clockwise when displayed.
                image.save(path, exif=exif)
        return path

    return generate


@pytest.fixture
def sources():
    """Register bytes with the source service; it supplies worker-reachable URLs."""
    with httpx.Client(
        base_url=os.environ.get("TEST_SOURCE_URL", "http://localhost:8090"),
        timeout=20, trust_env=False,
    ) as http:
        def register(content, name=None):
            name = name or f"{uuid.uuid4().hex}.png"
            response = http.put(f"/media/{name}", content=content)
            assert response.status_code == 201, response.text
            return response.json()

        yield register
