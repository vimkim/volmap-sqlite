"""Exercise the shipped listener with real HTTP, without third-party test clients."""
import contextlib
import http.client
import os
import pathlib
import re
import selectors
import shutil
import signal
import sqlite3
import subprocess
import sys
import tempfile
import time

BINARY = sys.argv[1]

@contextlib.contextmanager
def server(database, *args, trace=None):
    command = [BINARY, str(database), *args]
    if trace is not None:
        assert shutil.which("strace"), "strace is required for the no-outbound-connections contract"
        command = ["strace", "-f", "-e", "trace=connect", "-o", str(trace), *command]
    process = subprocess.Popen(command, stdout=subprocess.PIPE, stderr=subprocess.PIPE, start_new_session=True)
    logs = b""
    selector = selectors.DefaultSelector()
    selector.register(process.stderr, selectors.EVENT_READ)
    try:
        deadline = time.monotonic() + 10
        while not re.search(rb"Page atlas: [^\n]+\n", logs):
            assert time.monotonic() < deadline, logs
            assert selector.select(1), "no startup output"
            chunk = os.read(process.stderr.fileno(), 4096)
            assert chunk, logs
            logs += chunk
        url = re.search(rb"Page atlas: http://([^/]+)(/\S*)", logs)
        assert url, logs
        authority, entry = (part.decode() for part in url.groups())
        port = int(authority.rsplit(":", 1)[1])
        yield port, entry, logs
    finally:
        os.killpg(process.pid, signal.SIGINT)
        try:
            out, err = process.communicate(timeout=10)
        except subprocess.TimeoutExpired:
            os.killpg(process.pid, signal.SIGKILL)
            out, err = process.communicate()
            raise AssertionError("server failed to stop")
        selector.close()
        assert str(database.parent).encode() not in logs + out + err
        assert b"private-value-marker" not in logs + out + err


def request(port, path, method="GET", body=None, headers=None):
    connection = http.client.HTTPConnection("127.0.0.1", port, timeout=10)
    connection.request(method, path, body=body, headers=headers or {})
    response = connection.getresponse()
    result = response.status, dict(response.getheaders()), response.read()
    connection.close()
    return result

with tempfile.TemporaryDirectory() as directory:
    database = pathlib.Path(directory) / "private.sqlite"
    with sqlite3.connect(database) as connection:
        connection.executescript("CREATE TABLE data(value TEXT); INSERT INTO data VALUES ('private-value-marker');")
    with server(database, "--listen", "127.0.0.1:0") as (port, entry, logs):
        status, headers, body = request(port, entry)
        assert status == 200
        assert headers.get("cache-control") == "no-store"
        assert headers.get("x-content-type-options") == "nosniff", headers
        assert headers.get("referrer-policy") == "no-referrer"
        assert headers.get("x-frame-options") == "DENY"
        assert "default-src 'none'" in headers.get("content-security-policy", "")
        assert "connect-src 'self'" in headers["content-security-policy"]
        assert "'unsafe-inline'" not in headers["content-security-policy"]
        assert b"private-value-marker" not in body
        assert directory.encode() not in body

        # Every browser resource is embedded and fetched on this connection.
        assert entry.startswith("/sessions/")
        assert b"<script>" not in body
        resources = re.findall(rb'(?:src|href)="([^"]+)"', body)
        for resource in resources:
            assert resource.startswith(b"/") and not resource.startswith(b"//"), resource
            asset_status, asset_headers, asset = request(port, resource.decode())
            assert asset_status == 200, (resource, asset_status, asset)
            assert asset_headers.get("cache-control") == "no-store"
        assert "WARNING" not in logs.decode()
        bootstrap = request(port, entry + "/bootstrap.js")[2]
        snapshot = re.search(rb'snapshotId:"([^"]+)"', bootstrap)[1].decode()
        base = "/api/snapshots/" + snapshot
        deadline = time.monotonic() + 10
        import json
        while True:
            status, headers, body = request(port, base)
            state = json.loads(body)
            if state["state"] == "published":
                break
            assert time.monotonic() < deadline, state
            time.sleep(.01)
        origin = {"Origin": f"http://127.0.0.1:{port}", "Content-Type": "application/json"}
        for path, method, extra in [
            (base, "GET", {"Origin": "https://foreign.example"}),
            (base, "GET", {"Origin": "null"}),
            (base, "GET", {"Sec-Fetch-Site": "cross-site"}),
            (base, "GET", {"Sec-Fetch-Site": "same-site"}),
            (base, "GET", {"Host": f"rebind.example:{port}"}),
            (base, "GET", {"Host": "127.0.0.1:1"}),
            (base + "/cancel", "POST", {}),
            (base + "/cancel", "POST", {"Origin": "https://foreign.example"}),
            (base, "OPTIONS", {"Origin": "https://foreign.example"}),
        ]:
            status, headers, body = request(port, path, method, headers=extra)
            assert status == 403, (path, method, status, body)
            assert headers.get("cache-control") == "no-store"
            assert "access-control-allow-origin" not in headers
            assert b"private-value-marker" not in body
            assert snapshot.encode() not in body
        for path in [base + "?limit=1", base + "?source=/private/path", base + "?limit=-1"]:
            assert request(port, path)[0] == 400
        assert request(port, base + "?" + "x" * 600)[0] == 414
        assert request(port, base, headers={"X-Long": "x" * 9000})[0] == 431
        assert request(port, base + "/deep-inspections", "POST", b"x" * 4097, origin)[0] == 413
        for invalid in [b'{"target": "/private/path"}', b'{"target":', b'[]']:
            status, _, body = request(port, base + "/deep-inspections", "POST", invalid, origin)
            assert status in (400, 422), (status, body)
            assert b"/private/path" not in body
        for path in ["/sessions/foreign", "/sessions/foreign/bootstrap.js", "/api/snapshots/foreign", base + "/revisions/999", base + "/revisions/not-a-number", base + "/deep-inspections/foreign"]:
            status, headers, body = request(port, path)
            assert status in (400, 404), (path, status, body)
            assert headers.get("x-content-type-options") == "nosniff"
            assert b"not-a-number" not in body
            assert snapshot.encode() not in body
        graph = request(port, base + "/revisions/1")[2]
        assert b"private-value-marker" not in graph and directory.encode() not in graph
        target = {"sessionId": state["sessionId"], "snapshotId": snapshot, "revision": 1, "pageNumber": 2, "cellIndex": 0}
        for key, value in [("sessionId", "foreign"), ("snapshotId", "foreign"), ("pageNumber", 999), ("cellIndex", 999), ("revision", 999)]:
            invalid = dict(target, **{key: value})
            code, _, body = request(port, base + "/deep-inspections", "POST", json.dumps({"target": invalid}), origin)
            assert code in (404, 409), (invalid, code, body)
            assert b"foreign" not in body
            assert snapshot.encode() not in body
        # A request above the shared job ceiling reports a budget stop, not corruption.
        budget = {"maxPayloadBytes": 16777217, "maxOverflowPages": 32768, "maxValues": 4096}
        code, _, body = request(port, base + "/deep-inspections", "POST", json.dumps({"target": target, "budget": budget}), origin)
        assert json.loads(body)["state"] == "budget_stopped", (code, body)
        code, _, body = request(port, base + "/deep-inspections", "POST", json.dumps({"target": target}), origin)
        assert code == 202, (code, body)
        job = json.loads(body)
        job_url = base + "/deep-inspections/" + job["id"]
        deadline = time.monotonic() + 10
        while job["state"] == "pending":
            assert time.monotonic() < deadline, job
            job = json.loads(request(port, job_url)[2])
            time.sleep(.01)
        assert job["state"] == "completed", job
        for path in [base, base + "/revisions/1", base + "/revisions/2", job_url, base + "/evidence", entry]:
            _, headers, body = request(port, path)
            assert headers["cache-control"] == "no-store"
            assert b"private-value-marker" not in body and directory.encode() not in body
        assert request(port, job_url + "/result")[0] == 405
        assert request(port, job_url + "/result", "POST", json.dumps(dict(target, cellIndex=1)), origin)[0] == 409
        code, headers, body = request(port, job_url + "/result", "POST", json.dumps(target), origin)
        assert code == 200 and b"private-value-marker" in body, (code, body)
        assert headers["cache-control"] == "no-store"
        assert request(port, base + "/deep-inspections", "POST", json.dumps({"target": target}), origin)[0] == 409
        with server(database, "--listen", "127.0.0.1:0") as (other_port, _, _):
            assert request(other_port, base)[0] == 404
            assert request(other_port, entry)[0] == 404
            assert request(other_port, job_url)[0] == 404

    for flag, value, reason in [
        ("--max-web-response-bytes", "1024", "response_byte_budget"),
        ("--max-web-collection-items", "1", "response_collection_budget"),
    ]:
        with server(database, "--listen", "127.0.0.1:0", flag, value) as (port, entry, _):
            bootstrap = request(port, entry + "/bootstrap.js")[2]
            snapshot = re.search(rb'snapshotId:"([^"]+)"', bootstrap)[1].decode()
            base = "/api/snapshots/" + snapshot
            deadline = time.monotonic() + 10
            while True:
                code, headers, body = request(port, base + "/revisions/1")
                if code != 404:
                    break
                assert time.monotonic() < deadline
                time.sleep(.01)
            assert code == 507, (flag, code, body)
            assert json.loads(body) == {"state": "budget_stopped", "reason": reason}, body
            assert len(body) <= 1024 and b"private-value-marker" not in body
            assert headers["cache-control"] == "no-store"

    # The default listener is tested as launched, not inferred from CLI help.
    with server(database) as (port, entry, logs):
        assert port == 3000 and b"127.0.0.1:3000" in logs
        assert b"WARNING" not in logs
        assert request(port, entry)[0] == 200
    with server(database, "--listen", "0.0.0.0:0") as (port, entry, logs):
        assert b"WARNING" in logs and b"no built-in authentication or TLS" in logs
        assert b"Operator-controlled network protection is required" in logs
        assert request(port, entry)[0] == 200

    # Hold an admitted request at its body boundary; admission and timeout are observable.
    import socket
    with server(database, "--listen", "127.0.0.1:0", "--max-web-requests", "1") as (port, entry, _):
        slow = socket.create_connection(("127.0.0.1", port), timeout=10)
        slow.sendall((f"POST / HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://127.0.0.1:{port}\r\nContent-Length: 1\r\nConnection: close\r\n\r\n").encode())
        deadline = time.monotonic() + 3
        while True:
            code, _, body = request(port, entry)
            if code == 429:
                break
            assert time.monotonic() < deadline
            time.sleep(.01)
        assert json.loads(body)["reason"] == "concurrent_request_budget"
        response = http.client.HTTPResponse(slow)
        response.begin()
        assert response.status == 408
        assert json.loads(response.read())["reason"] == "request_body_timeout"
        slow.close()
        assert request(port, entry)[0] == 200

    trace = pathlib.Path(directory) / "network.trace"
    with server(database, "--listen", "127.0.0.1:0", trace=trace) as (port, entry, _):
        _, _, body = request(port, entry)
        for resource in re.findall(rb'(?:src|href)="([^"]+)"', body):
            assert request(port, resource.decode())[0] == 200
    assert "connect(" not in trace.read_text(), trace.read_text()

    with server(database, "--listen", "127.0.0.1:0") as (port, entry, _):
        held = [socket.create_connection(("127.0.0.1", port), timeout=5) for _ in range(64)]
        for stream in held:
            stream.sendall(b"GET / HTTP/1.1\r\n")
        waiting = socket.create_connection(("127.0.0.1", port), timeout=5)
        waiting.sendall(f"GET {entry} HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nConnection: close\r\n\r\n".encode())
        waiting.settimeout(.2)
        try:
            assert not waiting.recv(1), "connection cap admitted a 65th transport"
            raise AssertionError("unadmitted connection closed unexpectedly")
        except TimeoutError:
            pass
        for stream in held:
            stream.close()
        waiting.settimeout(5)
        response = http.client.HTTPResponse(waiting)
        response.begin()
        assert response.status == 200
        response.read()
        waiting.close()

    # Optional real-browser check supplements the required process syscall trace.
    browser = os.environ.get("VOLMAP_TEST_CHROMIUM")
    if browser:
        netlog = pathlib.Path(directory) / "browser-netlog.json"
        with server(database, "--listen", "127.0.0.1:0") as (port, entry, _):
            result = subprocess.run([browser, "--no-sandbox", "--disable-gpu", "--disable-background-networking", "--disable-component-update", "--no-first-run", "--dump-dom", "--virtual-time-budget=3000", f"--log-net-log={netlog}", f"http://127.0.0.1:{port}{entry}"], capture_output=True, timeout=15)
            assert result.returncode == 0, result.stderr
            assert b'data-page-number="2"' in result.stdout, (result.stdout, result.stderr)
            assert b"private-value-marker" not in result.stdout
            assert b"Refused to" not in result.stderr, result.stderr
            traffic = json.loads(netlog.read_text())
            urls = [event.get("params", {}).get("url", "") for event in traffic["events"]]
            assert any("/api/snapshots/" in url for url in urls), urls
            assert all(not url.startswith(("http:", "https:", "ws:", "wss:")) or url.startswith(f"http://127.0.0.1:{port}/") for url in urls), urls

    large_database = pathlib.Path(directory) / "large-selected.sqlite"
    with sqlite3.connect(large_database) as connection:
        connection.execute("CREATE TABLE data(value TEXT)")
        connection.execute("INSERT INTO data VALUES (?)", ("oversize-private-marker" * 2048,))
    # Sidecar metadata has its own path; neither its location nor main path may leak.
    pathlib.Path(str(large_database) + "-shm").write_bytes(b"\x00" * 32)
    with server(large_database, "--listen", "127.0.0.1:0", "--max-web-response-bytes", "8192") as (port, entry, _):
        bootstrap = request(port, entry + "/bootstrap.js")[2]
        snapshot = re.search(rb'snapshotId:"([^"]+)"', bootstrap)[1].decode()
        base = "/api/snapshots/" + snapshot
        deadline = time.monotonic() + 10
        while True:
            code, _, body = request(port, base)
            assert code == 200 and directory.encode() not in body, (code, body)
            state = json.loads(body)
            if state["state"] == "published":
                break
            assert time.monotonic() < deadline
            time.sleep(.01)
        target = {"sessionId": state["sessionId"], "snapshotId": snapshot, "revision": 1, "pageNumber": 2, "cellIndex": 0}
        origin = {"Origin": f"http://127.0.0.1:{port}", "Content-Type": "application/json"}
        code, _, body = request(port, base + "/deep-inspections", "POST", json.dumps({"target": target}), origin)
        assert code == 202, (code, body)
        job = json.loads(body)
        job_url = base + "/deep-inspections/" + job["id"]
        deadline = time.monotonic() + 10
        while job["state"] == "pending":
            assert time.monotonic() < deadline
            job = json.loads(request(port, job_url)[2])
            time.sleep(.01)
        assert job["state"] == "completed", job
        code, headers, body = request(port, job_url + "/result", "POST", json.dumps(target), origin)
        assert code == 507, (code, body)
        assert json.loads(body) == {"state": "budget_stopped", "reason": "response_byte_budget"}
        assert b"oversize-private-marker" not in body and directory.encode() not in body
        assert headers["cache-control"] == "no-store"
        assert json.loads(request(port, base)[2])["revision"] == 2

    failed = subprocess.run([BINARY, str(pathlib.Path(directory) / "missing.sqlite")], capture_output=True, timeout=10)
    assert failed.returncode != 0
    assert directory.encode() not in failed.stdout + failed.stderr
